---- MODULE model ----
EXTENDS Naturals, TLC

\* Abstract, per-run model of FlowRunMachine.

CONSTANTS
    Steps,
    MaxFailures

ASSUME Steps # {}
ASSUME MaxFailures \in Nat

RunStatuses == {"absent", "pending", "running", "completed", "failed", "canceled"}
TerminalRunStatuses == {"completed", "failed", "canceled"}
StepStatuses == {"none", "dispatched", "completed", "failed", "skipped", "canceled"}

VARIABLES
    runStatus,
    stepStatus,
    outputRecorded,
    failureCount

vars == << runStatus, stepStatus, outputRecorded, failureCount >>

Init ==
    /\ runStatus = "absent"
    /\ stepStatus = [s \in Steps |-> "none"]
    /\ outputRecorded = [s \in Steps |-> FALSE]
    /\ failureCount = 0

CreateRun ==
    /\ runStatus = "absent"
    /\ runStatus' = "pending"
    /\ UNCHANGED << stepStatus, outputRecorded, failureCount >>

StartRun ==
    /\ runStatus = "pending"
    /\ runStatus' = "running"
    /\ UNCHANGED << stepStatus, outputRecorded, failureCount >>

DispatchStep(s) ==
    /\ s \in Steps
    /\ runStatus = "running"
    /\ stepStatus[s] = "none"
    /\ stepStatus' = [stepStatus EXCEPT ![s] = "dispatched"]
    /\ UNCHANGED << runStatus, outputRecorded, failureCount >>

CompleteStep(s) ==
    /\ s \in Steps
    /\ runStatus = "running"
    /\ stepStatus[s] = "dispatched"
    /\ stepStatus' = [stepStatus EXCEPT ![s] = "completed"]
    /\ UNCHANGED << runStatus, outputRecorded, failureCount >>

RecordStepOutput(s) ==
    /\ s \in Steps
    /\ runStatus = "running"
    /\ stepStatus[s] = "completed"
    /\ ~outputRecorded[s]
    /\ outputRecorded' = [outputRecorded EXCEPT ![s] = TRUE]
    /\ UNCHANGED << runStatus, stepStatus, failureCount >>

FailStep(s) ==
    /\ s \in Steps
    /\ runStatus = "running"
    /\ stepStatus[s] = "dispatched"
    /\ failureCount < MaxFailures
    /\ stepStatus' = [stepStatus EXCEPT ![s] = "failed"]
    /\ failureCount' = failureCount + 1
    /\ UNCHANGED << runStatus, outputRecorded >>

SkipStep(s) ==
    /\ s \in Steps
    /\ runStatus = "running"
    /\ stepStatus[s] = "none"
    /\ stepStatus' = [stepStatus EXCEPT ![s] = "skipped"]
    /\ UNCHANGED << runStatus, outputRecorded, failureCount >>

CancelStep(s) ==
    /\ s \in Steps
    /\ runStatus = "running"
    /\ stepStatus[s] \in {"none", "dispatched"}
    /\ stepStatus' = [stepStatus EXCEPT ![s] = "canceled"]
    /\ UNCHANGED << runStatus, outputRecorded, failureCount >>

TerminalizeCompleted ==
    /\ runStatus = "running"
    /\ \A s \in Steps : stepStatus[s] \in {"completed", "skipped"}
    /\ runStatus' = "completed"
    /\ UNCHANGED << stepStatus, outputRecorded, failureCount >>

TerminalizeFailed ==
    /\ runStatus = "running"
    /\ \A s \in Steps : stepStatus[s] # "dispatched"
    /\ \E s \in Steps : stepStatus[s] = "failed"
    /\ runStatus' = "failed"
    /\ UNCHANGED << stepStatus, outputRecorded, failureCount >>

TerminalizeCanceled ==
    /\ runStatus = "running"
    /\ \A s \in Steps : stepStatus[s] # "dispatched"
    /\ \E s \in Steps : stepStatus[s] = "canceled"
    /\ runStatus' = "canceled"
    /\ UNCHANGED << stepStatus, outputRecorded, failureCount >>

TerminalStutter ==
    /\ runStatus \in TerminalRunStatuses
    /\ UNCHANGED vars

Next ==
    \/ CreateRun
    \/ StartRun
    \/ \E s \in Steps : DispatchStep(s)
    \/ \E s \in Steps : CompleteStep(s)
    \/ \E s \in Steps : RecordStepOutput(s)
    \/ \E s \in Steps : FailStep(s)
    \/ \E s \in Steps : SkipStep(s)
    \/ \E s \in Steps : CancelStep(s)
    \/ TerminalizeCompleted
    \/ TerminalizeFailed
    \/ TerminalizeCanceled
    \/ TerminalStutter

OutputOnlyForCompletedSteps ==
    \A s \in Steps :
        outputRecorded[s] => stepStatus[s] = "completed"

TerminalRunsHaveNoDispatchedSteps ==
    runStatus \in TerminalRunStatuses =>
        \A s \in Steps : stepStatus[s] # "dispatched"

CompletedRunHasOnlyCompletedOrSkippedSteps ==
    runStatus = "completed" =>
        \A s \in Steps : stepStatus[s] \in {"completed", "skipped"}

FailedRunHasEvidence ==
    runStatus = "failed" =>
        ((\E s \in Steps : stepStatus[s] = "failed") \/ failureCount > 0)

Spec == Init /\ [][Next]_vars

THEOREM Spec => []OutputOnlyForCompletedSteps
THEOREM Spec => []TerminalRunsHaveNoDispatchedSteps
THEOREM Spec => []CompletedRunHasOnlyCompletedOrSkippedSteps
THEOREM Spec => []FailedRunHasEvidence

=============================================================================
