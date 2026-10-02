---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkExecutionLifecycleMachine.

CONSTANTS BooleanValues, NatValues, StringValues, WorkExecutionEvidenceKindValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionWorkExecutionEvidenceKindValues == {None} \cup {Some(x) : x \in WorkExecutionEvidenceKindValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
Count(seq, value) == Cardinality({i \in DOMAIN seq : seq[i] = value})
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, binding_id, run_id, revision, last_failure_detail, evidence_kind

vars == << phase, model_step_count, binding_id, run_id, revision, last_failure_detail, evidence_kind >>

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ binding_id = ""
    /\ run_id = ""
    /\ revision = 0
    /\ last_failure_detail = None
    /\ evidence_kind = None

TerminalStutter ==
    /\ phase = "FlowFailed" \/ phase = "FlowCanceled" \/ phase = "EvidenceProjected" \/ phase = "WorkClosed" \/ phase = "LaunchFailed"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_45f559d421f5cafa == UNCHANGED << binding_id, run_id >>
UnchangedFrame_55e049da328809ca == UNCHANGED << binding_id, run_id, evidence_kind >>
UnchangedFrame_614ab550edf5e73d == UNCHANGED << binding_id, run_id, last_failure_detail, evidence_kind >>
UnchangedFrame_d0be54b2fc02a2da == UNCHANGED << binding_id, run_id, revision, last_failure_detail, evidence_kind >>

BindExecution(arg_binding_id, arg_run_id) ==
    /\ phase = "Absent"
    /\ ((arg_binding_id # "") /\ (arg_run_id # ""))
    /\ phase' = "LaunchRequested"
    /\ model_step_count' = model_step_count + 1
    /\ binding_id' = arg_binding_id
    /\ run_id' = arg_run_id
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = None
    /\ evidence_kind' = None


RecoverLaunchRequest ==
    /\ phase = "LaunchRequested"
    /\ phase' = "LaunchRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverUncertainLaunch ==
    /\ phase = "LaunchUncertain"
    /\ phase' = "LaunchUncertain"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverQuarantinedLaunch ==
    /\ phase = "LaunchQuarantined"
    /\ phase' = "LaunchQuarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverEvidenceProjection ==
    /\ phase = "EvidenceProjectionRequested"
    /\ phase' = "EvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverWorkClosure ==
    /\ phase = "WorkClosureRequested"
    /\ phase' = "WorkClosureRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverFlowFailureEvidenceProjection ==
    /\ phase = "FailureEvidenceProjectionRequested"
    /\ phase' = "FailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverFlowCancellationEvidenceProjection ==
    /\ phase = "CancellationEvidenceProjectionRequested"
    /\ phase' = "CancellationEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverLaunchFailureEvidenceProjection ==
    /\ phase = "LaunchFailureEvidenceProjectionRequested"
    /\ phase' = "LaunchFailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverFlowFailure ==
    /\ phase = "FlowFailed"
    /\ phase' = "FlowFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverFlowCancellation ==
    /\ phase = "FlowCanceled"
    /\ phase' = "FlowCanceled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverEvidenceProjected ==
    /\ phase = "EvidenceProjected"
    /\ phase' = "EvidenceProjected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverClosedWork ==
    /\ phase = "WorkClosed"
    /\ phase' = "WorkClosed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


RecoverLaunchFailure ==
    /\ phase = "LaunchFailed"
    /\ phase' = "LaunchFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


AcceptFlowLaunch ==
    /\ phase = "LaunchRequested" \/ phase = "LaunchUncertain" \/ phase = "LaunchQuarantined"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = None
    /\ evidence_kind' = None
    /\ UnchangedFrame_45f559d421f5cafa


ObserveRunningFlow ==
    /\ phase = "Running" \/ phase = "LaunchRequested" \/ phase = "LaunchUncertain" \/ phase = "LaunchQuarantined"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = None
    /\ evidence_kind' = None
    /\ UnchangedFrame_45f559d421f5cafa


ObserveCompletedFlow ==
    /\ phase = "Running" \/ phase = "LaunchRequested" \/ phase = "LaunchUncertain" \/ phase = "LaunchQuarantined"
    /\ phase' = "EvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = None
    /\ evidence_kind' = Some("Completed")
    /\ UnchangedFrame_45f559d421f5cafa


ObserveFailedFlow(detail) ==
    /\ phase = "Running" \/ phase = "LaunchRequested" \/ phase = "LaunchUncertain" \/ phase = "LaunchQuarantined"
    /\ phase' = "FailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = detail
    /\ evidence_kind' = Some("Failed")
    /\ UnchangedFrame_45f559d421f5cafa


ObserveCanceledFlow(detail) ==
    /\ phase = "Running" \/ phase = "LaunchRequested" \/ phase = "LaunchUncertain" \/ phase = "LaunchQuarantined"
    /\ phase' = "CancellationEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = detail
    /\ evidence_kind' = Some("Canceled")
    /\ UnchangedFrame_45f559d421f5cafa


ObserveLostRun(detail) ==
    /\ phase = "Running"
    /\ phase' = "FailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = Some(detail)
    /\ evidence_kind' = Some("RunLost")
    /\ UnchangedFrame_45f559d421f5cafa


ObserveLostCompletedRunBeforeEvidence(detail) ==
    /\ phase = "EvidenceProjectionRequested"
    /\ phase' = "FailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = Some(detail)
    /\ evidence_kind' = Some("RunLost")
    /\ UnchangedFrame_45f559d421f5cafa


RecordUncertainLaunch(detail) ==
    /\ phase = "LaunchRequested"
    /\ phase' = "LaunchUncertain"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = Some(detail)
    /\ evidence_kind' = None
    /\ UnchangedFrame_45f559d421f5cafa


QuarantineLaunch(detail) ==
    /\ phase = "LaunchRequested" \/ phase = "LaunchUncertain"
    /\ phase' = "LaunchQuarantined"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = Some(detail)
    /\ evidence_kind' = None
    /\ UnchangedFrame_45f559d421f5cafa


FailLaunch(detail) ==
    /\ phase = "LaunchRequested" \/ phase = "LaunchUncertain"
    /\ phase' = "LaunchFailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = Some(detail)
    /\ evidence_kind' = Some("LaunchFailed")
    /\ UnchangedFrame_45f559d421f5cafa


CommitLaunchFailureEvidenceProjection ==
    /\ phase = "LaunchFailureEvidenceProjectionRequested"
    /\ phase' = "LaunchFailed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_614ab550edf5e73d


CommitEvidenceProjection ==
    /\ phase = "EvidenceProjectionRequested"
    /\ phase' = "WorkClosureRequested"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_614ab550edf5e73d


CommitFlowFailureEvidenceProjection ==
    /\ phase = "FailureEvidenceProjectionRequested"
    /\ phase' = "FlowFailed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_614ab550edf5e73d


CommitFlowCancellationEvidenceProjection ==
    /\ phase = "CancellationEvidenceProjectionRequested"
    /\ phase' = "FlowCanceled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_614ab550edf5e73d


CommitWorkClosure ==
    /\ phase = "WorkClosureRequested"
    /\ phase' = "WorkClosed"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = None
    /\ UnchangedFrame_55e049da328809ca


RecordWorkClosureRefusal(detail) ==
    /\ phase = "WorkClosureRequested"
    /\ phase' = "EvidenceProjected"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ last_failure_detail' = Some(detail)
    /\ UnchangedFrame_55e049da328809ca


ClassifyRetryEligibilityTerminalFlowFailed ==
    /\ phase = "FlowFailed"
    /\ phase' = "FlowFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityTerminalFlowCanceled ==
    /\ phase = "FlowCanceled"
    /\ phase' = "FlowCanceled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityTerminalEvidenceProjected ==
    /\ phase = "EvidenceProjected"
    /\ phase' = "EvidenceProjected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityTerminalWorkClosed ==
    /\ phase = "WorkClosed"
    /\ phase' = "WorkClosed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityTerminalLaunchFailed ==
    /\ phase = "LaunchFailed"
    /\ phase' = "LaunchFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveAbsent ==
    /\ phase = "Absent"
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveLaunchRequested ==
    /\ phase = "LaunchRequested"
    /\ phase' = "LaunchRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveLaunchUncertain ==
    /\ phase = "LaunchUncertain"
    /\ phase' = "LaunchUncertain"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveLaunchQuarantined ==
    /\ phase = "LaunchQuarantined"
    /\ phase' = "LaunchQuarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveEvidenceProjectionRequested ==
    /\ phase = "EvidenceProjectionRequested"
    /\ phase' = "EvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveFailureEvidenceProjectionRequested ==
    /\ phase = "FailureEvidenceProjectionRequested"
    /\ phase' = "FailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveCancellationEvidenceProjectionRequested ==
    /\ phase = "CancellationEvidenceProjectionRequested"
    /\ phase' = "CancellationEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveLaunchFailureEvidenceProjectionRequested ==
    /\ phase = "LaunchFailureEvidenceProjectionRequested"
    /\ phase' = "LaunchFailureEvidenceProjectionRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


ClassifyRetryEligibilityLiveWorkClosureRequested ==
    /\ phase = "WorkClosureRequested"
    /\ phase' = "WorkClosureRequested"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_d0be54b2fc02a2da


Next ==
    \/ (phase = "Absent") /\ \E arg_binding_id \in StringValues : \E arg_run_id \in StringValues : BindExecution(arg_binding_id, arg_run_id)
    \/ RecoverLaunchRequest
    \/ RecoverUncertainLaunch
    \/ RecoverQuarantinedLaunch
    \/ RecoverRunning
    \/ RecoverEvidenceProjection
    \/ RecoverWorkClosure
    \/ RecoverFlowFailureEvidenceProjection
    \/ RecoverFlowCancellationEvidenceProjection
    \/ RecoverLaunchFailureEvidenceProjection
    \/ RecoverFlowFailure
    \/ RecoverFlowCancellation
    \/ RecoverEvidenceProjected
    \/ RecoverClosedWork
    \/ RecoverLaunchFailure
    \/ AcceptFlowLaunch
    \/ ObserveRunningFlow
    \/ ObserveCompletedFlow
    \/ (phase = "Running" \/ phase = "LaunchRequested" \/ phase = "LaunchUncertain" \/ phase = "LaunchQuarantined") /\ \E detail \in OptionStringValues : ObserveFailedFlow(detail)
    \/ (phase = "Running" \/ phase = "LaunchRequested" \/ phase = "LaunchUncertain" \/ phase = "LaunchQuarantined") /\ \E detail \in OptionStringValues : ObserveCanceledFlow(detail)
    \/ (phase = "Running") /\ \E detail \in StringValues : ObserveLostRun(detail)
    \/ (phase = "EvidenceProjectionRequested") /\ \E detail \in StringValues : ObserveLostCompletedRunBeforeEvidence(detail)
    \/ (phase = "LaunchRequested") /\ \E detail \in StringValues : RecordUncertainLaunch(detail)
    \/ (phase = "LaunchRequested" \/ phase = "LaunchUncertain") /\ \E detail \in StringValues : QuarantineLaunch(detail)
    \/ (phase = "LaunchRequested" \/ phase = "LaunchUncertain") /\ \E detail \in StringValues : FailLaunch(detail)
    \/ CommitLaunchFailureEvidenceProjection
    \/ CommitEvidenceProjection
    \/ CommitFlowFailureEvidenceProjection
    \/ CommitFlowCancellationEvidenceProjection
    \/ CommitWorkClosure
    \/ (phase = "WorkClosureRequested") /\ \E detail \in StringValues : RecordWorkClosureRefusal(detail)
    \/ ClassifyRetryEligibilityTerminalFlowFailed
    \/ ClassifyRetryEligibilityTerminalFlowCanceled
    \/ ClassifyRetryEligibilityTerminalEvidenceProjected
    \/ ClassifyRetryEligibilityTerminalWorkClosed
    \/ ClassifyRetryEligibilityTerminalLaunchFailed
    \/ ClassifyRetryEligibilityLiveAbsent
    \/ ClassifyRetryEligibilityLiveLaunchRequested
    \/ ClassifyRetryEligibilityLiveLaunchUncertain
    \/ ClassifyRetryEligibilityLiveLaunchQuarantined
    \/ ClassifyRetryEligibilityLiveRunning
    \/ ClassifyRetryEligibilityLiveEvidenceProjectionRequested
    \/ ClassifyRetryEligibilityLiveFailureEvidenceProjectionRequested
    \/ ClassifyRetryEligibilityLiveCancellationEvidenceProjectionRequested
    \/ ClassifyRetryEligibilityLiveLaunchFailureEvidenceProjectionRequested
    \/ ClassifyRetryEligibilityLiveWorkClosureRequested
    \/ TerminalStutter

identified_after_bind == (IF (phase = "Absent") THEN TRUE ELSE ((binding_id # "") /\ (run_id # "")))
evidence_projection_is_typed == (IF ((phase # "EvidenceProjectionRequested") /\ (phase # "FailureEvidenceProjectionRequested") /\ (phase # "CancellationEvidenceProjectionRequested") /\ (phase # "LaunchFailureEvidenceProjectionRequested")) THEN TRUE ELSE (evidence_kind # None))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []identified_after_bind
THEOREM Spec => []evidence_projection_is_typed

=============================================================================
