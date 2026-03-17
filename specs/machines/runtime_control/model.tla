---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for RuntimeControlMachine.

CONSTANTS AdmissionEffectValues, ContentShapeValues, HandlingModeValues, RequestIdValues, ReservationKeyValues, RunIdValues, StringValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionRequestIdValues == {None} \cup {Some(x) : x \in RequestIdValues}
OptionReservationKeyValues == {None} \cup {Some(x) : x \in ReservationKeyValues}

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


SubmitWorkFromIdle(work_id, content_shape, handling_mode, request_id, reservation_key) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


SubmitWorkFromRunning(work_id, content_shape, handling_mode, request_id, reservation_key) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionAcceptedIdleQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect) ==
    /\ phase = "Idle"
    /\ (handling_mode = "Queue")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = FALSE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedIdleSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect) ==
    /\ phase = "Idle"
    /\ (handling_mode = "Steer")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = TRUE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedRunningQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect) ==
    /\ phase = "Running"
    /\ (handling_mode = "Queue")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedRunningSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect) ==
    /\ phase = "Running"
    /\ (handling_mode = "Steer")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = TRUE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionRejectedIdle(work_id, reason) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionRejectedRunning(work_id, reason) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionDeduplicatedIdle(work_id, existing_work_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


AdmissionDeduplicatedRunning(work_id, existing_work_id) ==
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
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in RequestIdValues : \E reservation_key \in ReservationKeyValues : SubmitWorkFromIdle(work_id, content_shape, handling_mode, request_id, reservation_key)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in RequestIdValues : \E reservation_key \in ReservationKeyValues : SubmitWorkFromRunning(work_id, content_shape, handling_mode, request_id, reservation_key)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in RequestIdValues : \E reservation_key \in ReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedIdleQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in RequestIdValues : \E reservation_key \in ReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedIdleSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in RequestIdValues : \E reservation_key \in ReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedRunningQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in RequestIdValues : \E reservation_key \in ReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedRunningSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E reason \in {"alpha", "beta"} : AdmissionRejectedIdle(work_id, reason)
    \/ \E work_id \in WorkIdValues : \E reason \in {"alpha", "beta"} : AdmissionRejectedRunning(work_id, reason)
    \/ \E work_id \in WorkIdValues : \E existing_work_id \in WorkIdValues : AdmissionDeduplicatedIdle(work_id, existing_work_id)
    \/ \E work_id \in WorkIdValues : \E existing_work_id \in WorkIdValues : AdmissionDeduplicatedRunning(work_id, existing_work_id)
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
