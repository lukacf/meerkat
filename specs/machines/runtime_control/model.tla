---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for RuntimeControlMachine.

CONSTANTS AdmissionEffectValues, BooleanValues, CandidateIdValues, InputIdValues, InputKindValues, RunIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]
MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, current_run_id, pre_run_state, wake_pending, process_pending

vars == << phase, model_step_count, current_run_id, pre_run_state, wake_pending, process_pending >>

Init ==
    /\ phase = "Initializing"
    /\ model_step_count = 0
    /\ current_run_id = None
    /\ pre_run_state = None
    /\ wake_pending = FALSE
    /\ process_pending = FALSE

TerminalStutter ==
    /\ phase = "Stopped" \/ phase = "Destroyed"
    /\ UNCHANGED vars

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


BeginRunFromIdle(run_id) ==
    /\ phase = "Idle"
    /\ (current_run_id = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_state' = Some("Idle")
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


BeginRunFromRetired(run_id) ==
    /\ phase = "Retired"
    /\ (current_run_id = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_state' = Some("Retired")
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


RunCompleted(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunFailed(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCancelled(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RecoverRequestedFromIdle ==
    /\ phase = "Idle"
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ pre_run_state' = Some("Idle")
    /\ UNCHANGED << current_run_id, wake_pending, process_pending >>


RecoverRequestedFromRunning ==
    /\ phase = "Running"
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = Some("Running")
    /\ UNCHANGED << wake_pending, process_pending >>


RecoverySucceeded ==
    /\ phase = "Recovering"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


RetireRequestedFromIdle ==
    /\ phase = "Idle"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


RetireRequestedFromRunning ==
    /\ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


ResetRequested ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Recovering" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


StopRequested ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Running" \/ phase = "Recovering" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


DestroyRequested ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Running" \/ phase = "Recovering" \/ phase = "Retired" \/ phase = "Stopped"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


ResumeRequested ==
    /\ phase = "Recovering"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


SubmitCandidateFromIdle(candidate_id, candidate_kind) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


SubmitCandidateFromRunning(candidate_id, candidate_kind) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionAcceptedIdleNone(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Idle"
    /\ (wake = FALSE)
    /\ (process = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = wake
    /\ process_pending' = process
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedIdleWake(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Idle"
    /\ (wake = TRUE)
    /\ (process = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = FALSE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedIdleProcess(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Idle"
    /\ (wake = FALSE)
    /\ (process = TRUE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = FALSE
    /\ process_pending' = TRUE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedIdleWakeAndProcess(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Idle"
    /\ (wake = TRUE)
    /\ (process = TRUE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = TRUE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedRunningNone(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Running"
    /\ (wake = FALSE)
    /\ (process = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedRunningWake(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Running"
    /\ (wake = TRUE)
    /\ (process = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = FALSE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedRunningProcess(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Running"
    /\ (wake = FALSE)
    /\ (process = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = FALSE
    /\ process_pending' = TRUE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedRunningWakeAndProcess(candidate_id, candidate_kind, admission_effect, wake, process) ==
    /\ phase = "Running"
    /\ (wake = TRUE)
    /\ (process = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = TRUE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionRejectedIdle(candidate_id, reason) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionRejectedRunning(candidate_id, reason) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionDeduplicatedIdle(candidate_id, existing_input_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionDeduplicatedRunning(candidate_id, existing_input_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


ExternalToolDeltaReceivedIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


ExternalToolDeltaReceivedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


ExternalToolDeltaReceivedRecovering ==
    /\ phase = "Recovering"
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


ExternalToolDeltaReceivedRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


Next ==
    \/ Initialize
    \/ \E run_id \in RunIdValues : BeginRunFromIdle(run_id)
    \/ \E run_id \in RunIdValues : BeginRunFromRetired(run_id)
    \/ \E run_id \in RunIdValues : RunCompleted(run_id)
    \/ \E run_id \in RunIdValues : RunFailed(run_id)
    \/ \E run_id \in RunIdValues : RunCancelled(run_id)
    \/ RecoverRequestedFromIdle
    \/ RecoverRequestedFromRunning
    \/ RecoverySucceeded
    \/ RetireRequestedFromIdle
    \/ RetireRequestedFromRunning
    \/ ResetRequested
    \/ StopRequested
    \/ DestroyRequested
    \/ ResumeRequested
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : SubmitCandidateFromIdle(candidate_id, candidate_kind)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : SubmitCandidateFromRunning(candidate_id, candidate_kind)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedIdleNone(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedIdleWake(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedIdleProcess(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedIdleWakeAndProcess(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedRunningNone(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedRunningWake(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedRunningProcess(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E candidate_kind \in InputKindValues : \E admission_effect \in AdmissionEffectValues : \E wake \in BOOLEAN : \E process \in BOOLEAN : AdmissionAcceptedRunningWakeAndProcess(candidate_id, candidate_kind, admission_effect, wake, process)
    \/ \E candidate_id \in CandidateIdValues : \E reason \in {"alpha", "beta"} : AdmissionRejectedIdle(candidate_id, reason)
    \/ \E candidate_id \in CandidateIdValues : \E reason \in {"alpha", "beta"} : AdmissionRejectedRunning(candidate_id, reason)
    \/ \E candidate_id \in CandidateIdValues : \E existing_input_id \in InputIdValues : AdmissionDeduplicatedIdle(candidate_id, existing_input_id)
    \/ \E candidate_id \in CandidateIdValues : \E existing_input_id \in InputIdValues : AdmissionDeduplicatedRunning(candidate_id, existing_input_id)
    \/ ExternalToolDeltaReceivedIdle
    \/ ExternalToolDeltaReceivedRunning
    \/ ExternalToolDeltaReceivedRecovering
    \/ ExternalToolDeltaReceivedRetired
    \/ TerminalStutter

running_implies_active_run == ((phase # "Running") \/ (current_run_id # None))
active_run_only_while_running_or_retired == ((current_run_id = None) \/ (phase = "Running") \/ (phase = "Retired"))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []running_implies_active_run
THEOREM Spec => []active_run_only_while_running_or_retired

=============================================================================
