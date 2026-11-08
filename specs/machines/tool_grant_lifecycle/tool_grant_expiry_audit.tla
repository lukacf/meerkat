---- MODULE tool_grant_expiry_audit ----
\* Hand-written bounded audit of scoped tool grant expiry and run scope over
\* the generated ToolGrantLifecycleMachine model.
\*
\* One grant is issued with a fixed deadline, bounded by its window only or
\* scoped to a run of the session. The generated machine variables ARE its
\* durable row: the grant service holds no state, and every adjudication (a
\* model-request boundary, a gated dispatch, a list or audit read, a revoke,
\* a deadline timer fire, a run end, a recovery) rebuilds the machine from
\* the row and applies one generated input at the host clock it reads. A
\* crash therefore loses only process-local state: the session's projection
\* of the gated name (visible, visible_at), the armed deadline timer, and any
\* expiry or run end a dispatch decided but has not written yet.
\*
\* Adjudication is linearized at the row read: a boundary or a dispatch reads
\* the row and decides at one clock reading in a single step, as the service
\* reads the clock right after its row read and awaits nothing between that
\* reading and the hand-off. A run's end and its signal (EndRun) are one step:
\* the session awaits the signal before its run returns, so no other run of
\* the session starts in between. A signal that fails, or that a crash or a
\* dropped run never sends, is lost.
\*
\* Explored, at every interleaving: issuing the grant for its window from any
\* process, or for the session's latest run from the serving process (the run
\* it reads as current may have ended since); the session starting two runs
\* one after another, each run ending with its end signalled or lost; the
\* host clock advancing one millisecond at a time and rolling back once (the
\* clock is host-trusted, not rollback-resistant); the served session's
\* deadline timer firing early or due (Expire); a model-request boundary of
\* the current run applying AdmitUse with that run, writing any expiry or run
\* end it observes before it decides, and showing the name only on
\* admission; a gated dispatch of the current run, or a host call outside any
\* run, deciding from its own fresh AdmitUse right before the hand-off, so
\* the admission time is the hand-off time, and writing an expiry or run end
\* it observed only after it refused the call, in a separate step that can
\* fail or be lost to a crash; an operator revoke from any process; the end
\* of the grant's subject (the identity it was issued to ended) from any
\* process; a read from any process settling a due grant (Expire) without
\* loading the session; the serving process crashing (interrupting its
\* current run) and restarting, the restored gate starting closed; and
\* durable-tail recovery closing the interrupted run (EndRun) before the
\* session runs again, or losing that write.
\*
\* Properties:
\*
\*   AuditNoAdmitAtOrAfterDeadline: no gated call is handed on at or after
\*     the deadline.
\*   AuditTerminalNeverAdmits: no gated call is handed on once the grant
\*     expired, was revoked (by an operator or with its subject) or ended
\*     with its run.
\*   AuditTerminalLatched (action property): an ended grant never changes
\*     again, across crash, restart and clock rollback. It covers persisted
\*     ends only: an end a dispatch decided but has not written (or whose
\*     write was lost) is not an end of the row, and a call after a rollback
\*     before the deadline may still be admitted then.
\*   AuditExpiryNotEarly: an expired grant was observed at or after its
\*     deadline.
\*   AuditRevokedBeforeDeadline: a revoked grant ended before its deadline.
\*   AuditVisibleOnlyWhileLive: while the gated name is visible, the
\*     boundary that showed it ran before the deadline.
\*   AuditRunScopedUseInItsRun: a run-scoped grant hands on calls of its
\*     own run only: never a call of another run, never a call outside any
\*     run.
\*   AuditRunEndOnlyAfterItsRun: a run-scoped grant never ends with its run
\*     while that run goes on.
\*
\* AuditMutant selects a seeded shell defect (tool_grant_expiry_audit.sh
\* seeds the machine defects into a copy of the generated model instead):
\*   "dispatch_reuses_boundary_verdict": the dispatch admits a call because
\*     the boundary showed the name, without a fresh AdmitUse.
\*   "restart_reactivates_terminal": a restart reloads an ended grant as
\*     live.
\*   "revoked_admits": the dispatch hands on a call the revoked grant
\*     refused.
\*   "subject_ended_admits": the dispatch hands on a call the grant that
\*     ended with its subject refused.
\*   "boundary_trusts_durable_phase": the boundary shows the name because
\*     the stored row is still Active, without applying AdmitUse at the
\*     current clock.
\*   "other_run_admits": the dispatch hands on a call of another run that
\*     the run-scoped grant refused.
\*   "run_end_signalled_while_running": the session signals the end of its
\*     current run while the run goes on.
EXTENDS model

CONSTANTS AuditMaxSteps, AuditDeadline, AuditMaxLifetime, AuditClockMax, AuditGrantId,
          AuditMutant

ASSUME AuditDeadline \in 1..AuditClockMax
ASSUME AuditMaxLifetime >= AuditDeadline
ASSUME AuditMutant \in {"none", "dispatch_reuses_boundary_verdict",
                        "restart_reactivates_terminal", "revoked_admits",
                        "subject_ended_admits", "boundary_trusts_durable_phase",
                        "other_run_admits", "run_end_signalled_while_running"}

\* The session's runs, in start order, and the label of no run.
AuditRunIds == <<"run_1", "run_2">>
AuditNoRun == "none"

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
    admitted_after_end, rolled_back_after_end,
    \* Process-local: the (reading, run) pairs at which a dispatch refused a
    \* call it found expired or ended with its run, and has not written that
    \* end yet. History: whether such a write was lost, and whether a call was
    \* handed on after one was.
    pending_writes, write_lost, admitted_after_lost_write,
    \* The session's current run (AuditNoRun between runs), how many runs
    \* started, and the run a crash interrupted, which recovery closes before
    \* the session runs again.
    current_run, runs_started, interrupted_run,
    \* History: whether a gated call was handed on outside the grant's run
    \* scope, what ended the grant with its run, whether a call outside any
    \* run was refused, whether the grant ended with its run while that run
    \* went on, and whether the grant's subject ended after the grant ended
    \* with its run.
    admitted_outside_scope, run_ended_by, refused_outside_run, ended_while_running,
    subject_end_after_run_end

audit_vars == << clock, rollback_used, up, visible, visible_at, timer, admissions,
                 refused_before_deadline, last_dispatch, expired_by, down_at_deadline,
                 admitted_after_end, rolled_back_after_end >>

write_vars == << pending_writes, write_lost, admitted_after_lost_write >>

session_vars == << current_run, runs_started, interrupted_run >>

run_history_vars == << admitted_outside_scope, run_ended_by, refused_outside_run,
                       ended_while_running, subject_end_after_run_end >>

all_vars == << vars, audit_vars, write_vars, session_vars, run_history_vars >>

\* The grant ended. Expired, Revoked and RunEnded are terminal.
Ended == phase = "Expired" \/ phase = "Revoked" \/ phase = "RunEnded"

\* An option is read through its tag before its value. TLC orders record
\* fields by the order it first meets their names, and this module is parsed
\* first: a `value` met before `tag` makes it compare None's string value with
\* Some's number first, which it refuses.
AuditIsSome(o) == o.tag = "some"

\* The run a use of the current run names: none between runs.
CurrentRunArg == IF current_run = AuditNoRun THEN None ELSE Some(current_run)

\* This step ends the grant with its run while that run goes on. It reads
\* the session's run after the step, so every action evaluates it last.
EndsWhileRunning ==
    /\ phase # "RunEnded"
    /\ phase' = "RunEnded"
    /\ AuditIsSome(run_scope)
    /\ run_scope.value = current_run'

TrackRunEnd == ended_while_running' = (ended_while_running \/ EndsWhileRunning)

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
    /\ pending_writes = {}
    /\ write_lost = FALSE
    /\ admitted_after_lost_write = FALSE
    /\ current_run = AuditNoRun
    /\ runs_started = 0
    /\ interrupted_run = AuditNoRun
    /\ admitted_outside_scope = FALSE
    /\ run_ended_by = "none"
    /\ refused_outside_run = FALSE
    /\ ended_while_running = FALSE
    /\ subject_end_after_run_end = FALSE

\* The scopes an operator can issue the grant with: its window from any
\* process, or the session's latest run from the serving process, which
\* reads that run as the current one (it may have ended since).
IssueScopes ==
    {None} \cup (IF up /\ runs_started > 0 THEN {Some(AuditRunIds[runs_started])} ELSE {})

\* An operator issues the grant; a past or empty window is refused. A served
\* session's deadline timer is armed by the grant.
AuditIssue ==
    /\ \E scope \in IssueScopes :
        \/ Issue(AuditGrantId, clock, AuditDeadline, AuditMaxLifetime, scope)
        \/ IssueRefusedWindow(AuditGrantId, clock, AuditDeadline, AuditMaxLifetime, scope)
    /\ timer' = (up /\ phase' = "Active")
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, admissions,
                    refused_before_deadline, last_dispatch, expired_by, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

\* The host clock advances one millisecond.
AuditTick ==
    /\ clock < AuditClockMax
    /\ clock' = clock + 1
    /\ down_at_deadline' = (down_at_deadline \/ (~up /\ clock' = AuditDeadline))
    /\ UNCHANGED vars
    /\ UNCHANGED << rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, expired_by, admitted_after_end,
                    rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

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
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

\* The served session starts its next run, once the run a crash interrupted
\* is closed.
AuditStartRun ==
    /\ up
    /\ current_run = AuditNoRun
    /\ interrupted_run = AuditNoRun
    /\ runs_started < Len(AuditRunIds)
    /\ current_run' = AuditRunIds[runs_started + 1]
    /\ runs_started' = runs_started + 1
    /\ UNCHANGED interrupted_run
    /\ UNCHANGED vars
    /\ UNCHANGED audit_vars
    /\ UNCHANGED write_vars
    /\ UNCHANGED run_history_vars

\* The grant service applies EndRun for the run r at the current clock: it
\* ends a grant of that run, records the expiry of one past its deadline and
\* leaves any other grant as it is. Or the signal is lost, and nothing
\* changes.
EndRunSignalled(r, by) ==
    \/ /\ EndRunMatching(r, clock)
       /\ run_ended_by' = by
       /\ UNCHANGED expired_by
    \/ /\ EndRunAfterDeadline(r, clock)
       /\ expired_by' = by
       /\ UNCHANGED run_ended_by
    \/ /\ \/ EndRunOther(r, clock)
          \/ EndRunSettledExpired(r, clock)
          \/ EndRunSettledRevoked(r, clock)
          \/ EndRunSettledRunEnded(r, clock)
       /\ UNCHANGED << expired_by, run_ended_by >>
    \/ /\ UNCHANGED vars
       /\ UNCHANGED << expired_by, run_ended_by >>

\* The current run ends (completed, failed or cancelled), and the session
\* signals its end before the run returns, or loses the signal.
AuditFinishRun ==
    /\ up
    /\ current_run # AuditNoRun
    /\ EndRunSignalled(current_run, "run_end")
    /\ current_run' = AuditNoRun
    /\ UNCHANGED << runs_started, interrupted_run >>
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED << admitted_outside_scope, refused_outside_run, subject_end_after_run_end >>
    /\ TrackRunEnd

\* Seeded defect "run_end_signalled_while_running": the session signals the
\* end of its current run while the run goes on.
AuditEarlyRunEndSignal ==
    /\ AuditMutant = "run_end_signalled_while_running"
    /\ up
    /\ current_run # AuditNoRun
    /\ EndRunSignalled(current_run, "run_end")
    /\ UNCHANGED session_vars
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED << admitted_outside_scope, refused_outside_run, subject_end_after_run_end >>
    /\ TrackRunEnd

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
       \/ /\ \/ ExpireSettledExpired(clock)
             \/ ExpireSettledRevoked(clock)
             \/ ExpireSettledRunEnded(clock)
          /\ timer' = FALSE
          /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

\* A model-request boundary of the session's current run: the service
\* applies AdmitUse at the current clock with that run, writes any expiry or
\* run end it observes before it decides (a failed write closes the gate, as
\* a refusal does), and the session shows the gated name only on
\* UseAdmitted. Every boundary arms the timer while the grant is live.
AuditBoundary ==
    /\ up
    /\ current_run # AuditNoRun
    /\ IF AuditMutant = "boundary_trusts_durable_phase"
       THEN /\ visible' = (phase = "Active")
            /\ visible_at' = clock
            /\ UNCHANGED vars
            /\ UNCHANGED << expired_by, run_ended_by >>
       ELSE \/ /\ AdmitLive(clock, CurrentRunArg)
               /\ visible' = TRUE
               /\ visible_at' = clock
               /\ UNCHANGED << expired_by, run_ended_by >>
            \/ /\ AdmitDue(clock, CurrentRunArg)
               /\ visible' = FALSE
               /\ visible_at' = 0
               /\ expired_by' = "boundary"
               /\ UNCHANGED run_ended_by
            \/ /\ AdmitOtherRun(clock, CurrentRunArg)
               /\ visible' = FALSE
               /\ visible_at' = 0
               /\ run_ended_by' = "later_run"
               /\ UNCHANGED expired_by
            \/ /\ \/ AdmitEndedExpired(clock, CurrentRunArg)
                  \/ AdmitEndedRevoked(clock, CurrentRunArg)
                  \/ AdmitEndedRunEnded(clock, CurrentRunArg)
                  \/ AdmitEndedWithSubject(clock, CurrentRunArg)
               /\ visible' = FALSE
               /\ visible_at' = 0
               /\ UNCHANGED << expired_by, run_ended_by >>
            \/ /\ phase = "Unissued"
               /\ visible' = FALSE
               /\ visible_at' = 0
               /\ UNCHANGED vars
               /\ UNCHANGED << expired_by, run_ended_by >>
    /\ timer' = (phase' = "Active")
    /\ UNCHANGED << clock, rollback_used, up, admissions, refused_before_deadline,
                    last_dispatch, down_at_deadline, admitted_after_end,
                    rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED << admitted_outside_scope, refused_outside_run, subject_end_after_run_end >>
    /\ TrackRunEnd

\* The gated call naming the run `arg` is handed on at the current clock.
Admit(arg) ==
    /\ admissions' = admissions \cup {clock}
    /\ admitted_outside_scope' =
           (admitted_outside_scope \/ (AuditIsSome(run_scope) /\ arg # run_scope))
    /\ last_dispatch' = "admitted"
    /\ admitted_after_end' = (admitted_after_end \/ Ended)
    /\ admitted_after_lost_write' = (admitted_after_lost_write \/ write_lost)
    /\ UNCHANGED << refused_before_deadline, refused_outside_run >>

\* The gated call is refused at the current clock.
Refuse ==
    /\ refused_before_deadline' = (refused_before_deadline \/ clock < AuditDeadline)
    /\ last_dispatch' = "refused"
    /\ UNCHANGED << admissions, admitted_outside_scope, admitted_after_end,
                    admitted_after_lost_write >>

\* A refusal of a call that names a run, or of one the grant refuses for
\* another reason than its run.
RefuseInRun == Refuse /\ UNCHANGED refused_outside_run

\* A gated call of the session's current run, or a host call outside any
\* run: the dispatcher decides from its own AdmitUse at the current clock
\* after every other await of its entry, and hands the call on only on
\* UseAdmitted. Nothing awaits between the comparison and the hand-off. A
\* call the reading finds at or past the deadline, or of a run after the
\* grant's own, is refused before anything is written: the end's write
\* follows (AuditDispatchWrite) or is lost. The write of an expiry reads no
\* run (AdmitDue admits none), so it records none.
AuditDispatch ==
    /\ up
    /\ \E arg \in {CurrentRunArg, None} :
        IF AuditMutant = "dispatch_reuses_boundary_verdict"
        THEN /\ visible
             /\ UNCHANGED vars
             /\ Admit(arg)
             /\ UNCHANGED << expired_by, pending_writes >>
        ELSE \/ /\ AdmitLive(clock, arg)
                /\ Admit(arg)
                /\ UNCHANGED << expired_by, pending_writes >>
             \/ /\ ENABLED AdmitDue(clock, arg)
                /\ UNCHANGED vars
                /\ RefuseInRun
                /\ pending_writes' = pending_writes \cup {<<clock, None>>}
                /\ UNCHANGED expired_by
             \/ /\ ENABLED AdmitOtherRun(clock, arg)
                /\ UNCHANGED vars
                /\ IF AuditMutant = "other_run_admits"
                   THEN /\ Admit(arg)
                        /\ UNCHANGED pending_writes
                   ELSE /\ RefuseInRun
                        /\ pending_writes' = pending_writes \cup {<<clock, arg>>}
                /\ UNCHANGED expired_by
             \/ /\ AdmitOutsideRun(clock, arg)
                /\ Refuse
                /\ refused_outside_run' = TRUE
                /\ UNCHANGED << expired_by, pending_writes >>
             \/ /\ \/ AdmitEndedExpired(clock, arg)
                   \/ AdmitEndedRunEnded(clock, arg)
                /\ RefuseInRun
                /\ UNCHANGED << expired_by, pending_writes >>
             \/ /\ AdmitEndedRevoked(clock, arg)
                /\ IF AuditMutant = "revoked_admits" THEN Admit(arg) ELSE RefuseInRun
                /\ UNCHANGED << expired_by, pending_writes >>
             \/ /\ AdmitEndedWithSubject(clock, arg)
                /\ IF AuditMutant = "subject_ended_admits" THEN Admit(arg) ELSE RefuseInRun
                /\ UNCHANGED << expired_by, pending_writes >>
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, down_at_deadline,
                    rolled_back_after_end, write_lost >>
    /\ UNCHANGED session_vars
    /\ UNCHANGED << run_ended_by, ended_while_running, subject_end_after_run_end >>

\* The dispatch writes an end it decided, at the reading and with the run it
\* decided with, to the row as it now stands: the write reloads a row
\* another actor changed meanwhile and applies AdmitUse once more, which
\* leaves an ended row as it is.
AuditDispatchWrite ==
    /\ up
    /\ \E w \in pending_writes :
        /\ pending_writes' = pending_writes \ {w}
        /\ \/ /\ AdmitDue(w[1], w[2])
              /\ expired_by' = "dispatch"
              /\ UNCHANGED run_ended_by
           \/ /\ AdmitOtherRun(w[1], w[2])
              /\ run_ended_by' = "later_run"
              /\ UNCHANGED expired_by
           \/ /\ \/ AdmitEndedExpired(w[1], w[2])
                 \/ AdmitEndedRevoked(w[1], w[2])
                 \/ AdmitEndedRunEnded(w[1], w[2])
                 \/ AdmitEndedWithSubject(w[1], w[2])
              /\ UNCHANGED << expired_by, run_ended_by >>
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end, write_lost,
                    admitted_after_lost_write >>
    /\ UNCHANGED session_vars
    /\ UNCHANGED << admitted_outside_scope, refused_outside_run, subject_end_after_run_end >>
    /\ TrackRunEnd

\* A decided write fails: the refusal stands and the row stays as it was, so
\* the grant is not ended until another actor observes the end.
AuditDispatchWriteLost ==
    /\ \E w \in pending_writes : pending_writes' = pending_writes \ {w}
    /\ write_lost' = TRUE
    /\ UNCHANGED vars
    /\ UNCHANGED audit_vars
    /\ UNCHANGED admitted_after_lost_write
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

\* An operator revokes the grant from any process. A revoke at or after the
\* deadline records the expiry instead.
AuditRevoke ==
    /\ \/ /\ RevokeActive(clock)
          /\ UNCHANGED expired_by
       \/ /\ RevokeAfterDeadline(clock)
          /\ expired_by' = "revoke"
       \/ /\ \/ RevokeSettledExpired(clock)
             \/ RevokeSettledRevoked(clock)
             \/ RevokeSettledRunEnded(clock)
          /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

\* The grant's subject ends, from any process: the mob ends the identity it
\* was issued to (or the session that identity held). The grant ends with it,
\* revoked with the end reason SubjectEnded; an end at or after the deadline
\* records the expiry instead, and an ended grant, one that ended with its run
\* included, stays as it is.
AuditEndWithSubject ==
    /\ \/ /\ EndWithSubjectActive(clock)
          /\ UNCHANGED << expired_by, subject_end_after_run_end >>
       \/ /\ EndWithSubjectAfterDeadline(clock)
          /\ expired_by' = "subject"
          /\ UNCHANGED subject_end_after_run_end
       \/ /\ (EndWithSubjectSettledExpired(clock) \/ EndWithSubjectSettledRevoked(clock))
          /\ UNCHANGED << expired_by, subject_end_after_run_end >>
       \/ /\ EndWithSubjectSettledRunEnded(clock)
          /\ subject_end_after_run_end' = TRUE
          /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED << admitted_outside_scope, run_ended_by, refused_outside_run,
                    ended_while_running >>

\* A list or audit read from any process settles a due grant with Expire. It
\* touches only the grant row: it neither loads nor wakes the session.
AuditRead ==
    /\ \/ /\ ExpireDue(clock)
          /\ expired_by' = "read"
       \/ /\ \/ ExpireNotDue(clock)
             \/ ExpireSettledExpired(clock)
             \/ ExpireSettledRevoked(clock)
             \/ ExpireSettledRunEnded(clock)
          /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

\* The serving process dies. The durable row survives; the projection, the
\* armed timer and any end a dispatch decided but has not written are
\* process-local and are lost. The run it was running is interrupted, and
\* stays so (durably) until recovery closes it.
AuditCrash ==
    /\ up
    /\ up' = FALSE
    /\ visible' = FALSE
    /\ visible_at' = 0
    /\ timer' = FALSE
    /\ pending_writes' = {}
    /\ write_lost' = (write_lost \/ pending_writes # {})
    /\ current_run' = AuditNoRun
    /\ interrupted_run' = IF current_run # AuditNoRun THEN current_run ELSE interrupted_run
    /\ UNCHANGED runs_started
    /\ UNCHANGED vars
    /\ UNCHANGED << clock, rollback_used, admissions, refused_before_deadline,
                    last_dispatch, expired_by, down_at_deadline, admitted_after_end,
                    rolled_back_after_end, admitted_after_lost_write >>
    /\ UNCHANGED run_history_vars

\* A new process serves the session. Every build clears the open names it
\* restores, so the gate restores closed; nothing settles the row until the
\* first boundary, dispatch, read, timer fire or recovery.
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
            /\ UNCHANGED << model_step_count, grant_id, issued_at_ms, expires_at_ms, run_scope >>
       ELSE UNCHANGED vars
    /\ UNCHANGED << clock, rollback_used, admissions, refused_before_deadline,
                    last_dispatch, expired_by, down_at_deadline, admitted_after_end,
                    rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED session_vars
    /\ UNCHANGED run_history_vars

\* Durable-tail recovery closes the run the crash interrupted, before the
\* session runs again, and ends that run's run-scoped grant (EndRun); or it
\* closes the run and loses that write.
AuditRecover ==
    /\ up
    /\ interrupted_run # AuditNoRun
    /\ EndRunSignalled(interrupted_run, "recovery")
    /\ interrupted_run' = AuditNoRun
    /\ UNCHANGED << current_run, runs_started >>
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>
    /\ UNCHANGED write_vars
    /\ UNCHANGED << admitted_outside_scope, refused_outside_run, subject_end_after_run_end >>
    /\ TrackRunEnd

AuditNext ==
    \/ AuditIssue
    \/ AuditTick
    \/ AuditRollback
    \/ AuditStartRun
    \/ AuditFinishRun
    \/ AuditEarlyRunEndSignal
    \/ AuditTimerFire
    \/ AuditBoundary
    \/ AuditDispatch
    \/ AuditDispatchWrite
    \/ AuditDispatchWriteLost
    \/ AuditRevoke
    \/ AuditEndWithSubject
    \/ AuditRead
    \/ AuditCrash
    \/ AuditRestart
    \/ AuditRecover

AuditSpec == AuditInit /\ [][AuditNext]_all_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* --- Properties -------------------------------------------------------------

AuditNoAdmitAtOrAfterDeadline == \A t \in admissions : t < AuditDeadline

AuditTerminalNeverAdmits == ~admitted_after_end

AuditTerminalLatched ==
    [][Ended => /\ phase' = phase
               /\ ended_at_ms' = ended_at_ms
               /\ end_reason' = end_reason
               /\ run_scope' = run_scope]_all_vars

AuditExpiryNotEarly ==
    phase = "Expired" => (AuditIsSome(ended_at_ms) /\ ended_at_ms.value >= expires_at_ms)

AuditRevokedBeforeDeadline ==
    phase = "Revoked" => (AuditIsSome(ended_at_ms) /\ ended_at_ms.value < expires_at_ms)

AuditVisibleOnlyWhileLive == visible => visible_at < AuditDeadline

AuditRunScopedUseInItsRun == ~admitted_outside_scope

AuditRunEndOnlyAfterItsRun == ~ended_while_running

\* --- Reachability witnesses: each negation must be violated ----------------

\* The deadline timer observed the expiry.
NotAuditWitnessExpiredByTimer == ~(expired_by = "timer")

\* A boundary before the deadline showed the name, and the racing call
\* adjudicated at the deadline was refused and its expiry written.
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

\* A grant that ended with its subject refused a call.
NotAuditWitnessEndedWithSubjectThenRefused ==
    ~(phase = "Revoked" /\ AuditIsSome(end_reason) /\ end_reason.value = "SubjectEnded"
      /\ last_dispatch = "refused")

\* A revoke at the deadline recorded expiry, not revocation.
NotAuditWitnessRevokeAfterDeadlineRecordsExpiry == ~(expired_by = "revoke")

\* The subject's end at the deadline recorded expiry, as a revoke does.
NotAuditWitnessSubjectEndAfterDeadlineRecordsExpiry == ~(expired_by = "subject")

\* A dispatch refused a call at the deadline and its expiry write was lost,
\* so the row never ended; after the clock rolled back before the deadline,
\* a call was handed on. It was handed on before the deadline, so every
\* safety property still holds.
NotAuditWitnessUnpersistedRefusalThenAdmitted == ~admitted_after_lost_write

\* The session signalled the end of the grant's run, which ended the grant,
\* and a later call was refused.
NotAuditWitnessRunEndedThenRefused ==
    ~(phase = "RunEnded" /\ run_ended_by = "run_end" /\ last_dispatch = "refused")

\* The end of the grant's run was never signalled, and a use of the
\* session's next run ended the stale grant.
NotAuditWitnessLaterRunEndsStaleGrant ==
    ~(phase = "RunEnded" /\ run_ended_by = "later_run")

\* A host call outside any run was refused, and the run-scoped grant stayed
\* live for its run.
NotAuditWitnessHostCallOutsideRunRefusedWithoutEnding ==
    ~(refused_outside_run /\ phase = "Active" /\ AuditIsSome(run_scope))

\* Recovery closed the run a crash interrupted and ended its grant.
NotAuditWitnessRecoveryEndsInterruptedRun ==
    ~(phase = "RunEnded" /\ run_ended_by = "recovery")

\* The subject of a grant whose run already ended ended, and the grant stayed
\* ended with its run, its end reason unchanged.
NotAuditWitnessSubjectEndOfRunEndedGrantChangesNothing ==
    ~(subject_end_after_run_end /\ phase = "RunEnded"
      /\ AuditIsSome(end_reason) /\ end_reason.value = "RunEnded")
====
