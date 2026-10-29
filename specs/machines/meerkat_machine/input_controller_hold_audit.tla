---- MODULE input_controller_hold_audit ----
\* Hand-written bounded audit of the machine-owned controller holds.
\*
\* A resume of retained work that a graceful requeue stop kept, or that a
\* restart found, in the store comes back without its controller client (a
\* client never survives the store), and the runtime asks its host for the
\* client again. While the host cannot say yet, the machine holds the input
\* (`input_controller_holds`, HoldInputController): a held input is never
\* staged or applied and keeps its attempts, and the run marked resumable that
\* it is bound to does not resume. The host supplying the client releases the
\* hold (ReleaseInputController, which emits InputControllerReleased); every
\* transition that ends an input, or settles or releases its run, drops its
\* hold.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and explores,
\* for one session, two kept governed inputs a restart recovered through the
\* generated recovery arms: one queued, one staged and bound to the run the
\* registration marked resumable. In Idle, Attached and Running it drives the
\* hold and release arms for both inputs, the start of the marked run and of a
\* new one, the resume of the marked run, its settlement and release, the
\* staging of an input into the current run, the return of a staged input to
\* its lane, the apply of a staged input, and the abandon of either input as a
\* refused resume. Properties:
\*
\*   AuditHeldWorkNeverAdvances (action property): a step taken while an
\*     input is held never stages it, never applies it and never changes its
\*     attempt count.
\*   AuditHeldRunNeverResumes (action property): no step resumes the marked
\*     run (makes it current and clears its mark) while an input bound to it
\*     is held.
\*   AuditHoldAndReleaseTouchOnlyHolds (action property): a hold adds exactly
\*     its input to the holds and a release removes exactly its input, and
\*     neither changes an input phase, lane, run association or attempt
\*     count, the resumable mark or the current run.
\*
\* The generated invariant input_controller_holds_are_kept_unstaged_governed_inputs
\* (every held input is a recovered governed input, queued in a lane or bound,
\* Staged or Applied, to the marked run) is checked with every other generated
\* invariant, since the config is derived from the generated ci.cfg.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. The runner's
\* --mutants mode also checks that seven seeded model defects are refused (a
\* stage, an apply and a resume that ignore the hold, a settlement and an
\* abandon that keep it, a hold that ignores the input's phase, a release that
\* clears every hold).
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* input_controller_hold_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/input_controller_hold_audit.sh 16
\*
\* Scope limits: the re-ask timer, the hold's budget and the host's answers
\* are shell, covered by the meerkat-runtime rows in
\* local_authorization/tests/cold_continuation.rs and the plan and kernel rows
\* in meerkat_machine/retained_resume.rs. The no_live_boundary_delivery guard
\* of HoldInputController is not driven here (a recovered input carries no
\* live-boundary delivery), nor is the test-only IncrementAttemptCount.
EXTENDS model

CONSTANT AuditMaxSteps

VARIABLES
    audit_released,          \* inputs released at least once
    audit_held_abandoned,    \* an input ended while it was held
    audit_settled_held,      \* the marked run settled while its input was held
    audit_released_run_held  \* the marked run was released while its input was held

auditVars == <<audit_released, audit_held_abandoned, audit_settled_held,
               audit_released_run_held>>

AuditSession == "sessionid_1"
AuditQueued == "input_1"
AuditBound == "input_2"
AuditInputs == {AuditQueued, AuditBound}
AuditRunKept == "runid_1"
AuditRunNew == "runid_2"
AuditRuns == {AuditRunKept, AuditRunNew}
AuditPrefixLength == 9
AppliedPhases == {"Applied", "AppliedPendingConsumption"}

PhaseOf(i) == IF i \in DOMAIN input_phases THEN input_phases[i] ELSE "Absent"
Attempts(i) == IF i \in DOMAIN input_attempt_counts THEN input_attempt_counts[i] ELSE 0
Assoc(i) == IF i \in DOMAIN input_run_associations THEN Some(input_run_associations[i]) ELSE None

KeepHistory == UNCHANGED auditVars

\* Scripted restart: register, mark the kept run resumable, then recover the
\* queued input and the input bound to the marked run through the generated
\* recovery arms, each bound to its own governed association.
AuditPrefix ==
    \/ model_step_count = 0 /\ Initialize /\ KeepHistory
    \/ model_step_count = 1 /\ RegisterSessionIdle(AuditSession, None, {}) /\ KeepHistory
    \/ model_step_count = 2 /\ MarkRunResumableIdle(AuditRunKept) /\ KeepHistory
    \/ model_step_count = 3
       /\ RecoverAdmittedInputIdle(AuditQueued, "Prompt", "RunStart", "ContentTurn", None, "Queue")
       /\ KeepHistory
    \/ model_step_count = 4
       /\ RecoverInputLifecycleIdle(AuditQueued, "Queued", None, None, None, None, 0, 0,
             None, None, Some(1), None, Some("Queue"), Some("Queue"), Some("RunStart"),
             Some("ContentTurn"), None, TRUE)
       /\ KeepHistory
    \/ model_step_count = 5
       /\ BindInputAuthorityIdle(AuditQueued, "binding_1", "batch_1")
       /\ KeepHistory
    \/ model_step_count = 6
       /\ RecoverAdmittedInputIdle(AuditBound, "Prompt", "RunStart", "ContentTurn", None, "Queue")
       /\ KeepHistory
    \/ model_step_count = 7
       /\ RecoverInputLifecycleIdle(AuditBound, "Staged", None, None, None, None, 0, 1,
             Some(AuditRunKept), None, Some(2), None, Some("Queue"), None, None, None, None,
             FALSE)
       /\ KeepHistory
    \/ model_step_count = 8
       /\ BindInputAuthorityIdle(AuditBound, "binding_2", "batch_2")
       /\ KeepHistory

HoldFor(i) ==
    \/ HoldInputControllerIdle(i)
    \/ HoldInputControllerAttached(i)
    \/ HoldInputControllerRunning(i)
    \/ HoldInputControllerRetired(i)
    \/ HoldInputControllerStopped(i)

ReleaseFor(i) ==
    \/ ReleaseInputControllerIdle(i)
    \/ ReleaseInputControllerAttached(i)
    \/ ReleaseInputControllerRunning(i)
    \/ ReleaseInputControllerRetired(i)
    \/ ReleaseInputControllerStopped(i)

AbandonFor(i) ==
    \/ AbandonInputIdle(i, "ResumeRefused", Attempts(i))
    \/ AbandonInputAttached(i, "ResumeRefused", Attempts(i))
    \/ AbandonInputRunning(i, "ResumeRefused", Attempts(i))

SettleKept ==
    \/ SettleResumableRunIdle(AuditRunKept, "ResumeRefused")
    \/ SettleResumableRunAttached(AuditRunKept, "ResumeRefused")

ReleaseKept ==
    \/ ReleaseResumableRunIdle(AuditRunKept)
    \/ ReleaseResumableRunAttached(AuditRunKept)

AuditHold == \E i \in AuditInputs : HoldFor(i)
AuditRelease == \E i \in AuditInputs : ReleaseFor(i)

\* The run steps around a held input: the attachment, the start of the marked
\* run or of a new one (once the mark is gone), the resume of the marked run,
\* the staging of an input into the current run, a staged input's return to
\* its lane, and the apply of a staged input (the shell applies only the
\* staged inputs of a run).
AuditRunStep ==
    \/ EnsureSessionWithExecutorIdle(AuditSession)
    \/ \E r \in AuditRuns :
        \/ PrepareIdle(AuditSession, r)
        \/ PrepareAttached(AuditSession, r)
    \/ ResumeRunRunning(AuditRunKept)
    \/ \E i \in AuditInputs :
        \/ \E r \in AuditRuns : StageForRunRunning(i, r)
        \/ RollbackStagedIdle(i, "Queue")
        \/ RollbackStagedAttached(i, "Queue")
        \/ RollbackStagedRunning(i, "Queue")
        \/ /\ PhaseOf(i) = "Staged"
           /\ \/ MarkAppliedIdle(i)
              \/ MarkAppliedAttached(i)
              \/ MarkAppliedRunning(i)

HeldBound == AuditBound \in input_controller_holds

AuditBody ==
    \/ AuditHold /\ KeepHistory
    \/ \E i \in AuditInputs :
        /\ ReleaseFor(i)
        /\ audit_released' = audit_released \cup {i}
        /\ UNCHANGED <<audit_held_abandoned, audit_settled_held, audit_released_run_held>>
    \/ \E i \in AuditInputs :
        /\ AbandonFor(i)
        /\ audit_held_abandoned' = (audit_held_abandoned \/ i \in input_controller_holds)
        /\ UNCHANGED <<audit_released, audit_settled_held, audit_released_run_held>>
    \/ /\ SettleKept
       /\ audit_settled_held' = (audit_settled_held \/ HeldBound)
       /\ UNCHANGED <<audit_released, audit_held_abandoned, audit_released_run_held>>
    \/ /\ ReleaseKept
       /\ audit_released_run_held' = (audit_released_run_held \/ HeldBound)
       /\ UNCHANGED <<audit_released, audit_held_abandoned, audit_settled_held>>
    \/ AuditRunStep /\ KeepHistory

AuditNext ==
    \/ AuditPrefix
    \/ (model_step_count >= AuditPrefixLength /\ AuditBody)

AuditInit ==
    /\ Init
    /\ audit_released = {}
    /\ audit_held_abandoned = FALSE
    /\ audit_settled_held = FALSE
    /\ audit_released_run_held = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditHeldWorkNeverAdvances ==
    [][\A i \in AuditInputs :
         i \in input_controller_holds =>
            /\ (PhaseOf(i) # "Staged" => PhaseOf(i)' # "Staged")
            /\ (PhaseOf(i) \notin AppliedPhases => PhaseOf(i)' \notin AppliedPhases)
            /\ Attempts(i)' = Attempts(i)
      ]_<<vars, auditVars>>

AuditHeldRunNeverResumes ==
    [][\A r \in AuditRuns :
         (resumable_run_id = Some(r) /\ resumable_run_id' = None /\ current_run_id' = Some(r))
           => ~(\E i \in input_controller_holds : Assoc(i) = Some(r))
      ]_<<vars, auditVars>>

AuditHoldAndReleaseTouchOnlyHolds ==
    [][/\ \A i \in AuditInputs :
            /\ HoldFor(i) => input_controller_holds' = input_controller_holds \cup {i}
            /\ ReleaseFor(i) => input_controller_holds' = input_controller_holds \ {i}
       /\ (AuditHold \/ AuditRelease)
            => UNCHANGED <<input_phases, input_lane, input_run_associations,
                           input_attempt_counts, resumable_run_id, current_run_id>>
      ]_<<vars, auditVars>>

\* Reachability goals, each checked as an expected violation of its negation.
\* A held queued input waits while a new run of its session runs.
AuditWitnessHeldWhileRunCurrent ==
    /\ AuditQueued \in input_controller_holds
    /\ PhaseOf(AuditQueued) = "Queued"
    /\ current_run_id = Some(AuditRunNew)
\* Released, the queued input stages into a run.
AuditWitnessReleasedThenStaged ==
    /\ AuditQueued \in audit_released
    /\ PhaseOf(AuditQueued) = "Staged"
\* A held input ends as a refused resume, and its hold goes with it.
AuditWitnessHeldEndsRefused == audit_held_abandoned
\* The marked run starts but cannot resume while its input is held.
AuditWitnessHeldRunWaits ==
    /\ current_run_id = Some(AuditRunKept)
    /\ resumable_run_id = Some(AuditRunKept)
    /\ HeldBound
\* Released, the bound input lets its run resume.
AuditWitnessReleasedRunResumes ==
    /\ AuditBound \in audit_released
    /\ resumable_run_id = None
    /\ current_run_id = Some(AuditRunKept)
\* Settling the marked run drops its held input's hold.
AuditWitnessSettledDropsHold == audit_settled_held /\ ~HeldBound
\* Releasing the marked run drops the hold, and the input returns to its lane.
AuditWitnessReleasedRunRequeues ==
    /\ audit_released_run_held
    /\ ~HeldBound
    /\ PhaseOf(AuditBound) = "Queued"

NotAuditWitnessHeldWhileRunCurrent == ~AuditWitnessHeldWhileRunCurrent
NotAuditWitnessReleasedThenStaged == ~AuditWitnessReleasedThenStaged
NotAuditWitnessHeldEndsRefused == ~AuditWitnessHeldEndsRefused
NotAuditWitnessHeldRunWaits == ~AuditWitnessHeldRunWaits
NotAuditWitnessReleasedRunResumes == ~AuditWitnessReleasedRunResumes
NotAuditWitnessSettledDropsHold == ~AuditWitnessSettledDropsHold
NotAuditWitnessReleasedRunRequeues == ~AuditWitnessReleasedRunRequeues
====
