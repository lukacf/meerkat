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


AttachFromIdle ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


DetachToIdle ==
    /\ phase = "Attached"
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


BeginRunFromAttached(run_id) ==
    /\ phase = "Attached"
    /\ (current_run_id = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_state' = Some("Attached")
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


BeginRunFromRecovering(run_id) ==
    /\ phase = "Recovering"
    /\ (current_run_id = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_state' = Some("Recovering")
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


RunCompletedToIdle(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ ((pre_run_state = None) \/ (pre_run_state = Some("Idle")) \/ (pre_run_state = Some("Recovering")))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCompletedToAttached(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ (pre_run_state = Some("Attached"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCompletedToRetired(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ (pre_run_state = Some("Retired"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunFailedToIdle(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ ((pre_run_state = None) \/ (pre_run_state = Some("Idle")) \/ (pre_run_state = Some("Recovering")))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunFailedToAttached(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ (pre_run_state = Some("Attached"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunFailedToRetired(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ (pre_run_state = Some("Retired"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCancelledToIdle(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ ((pre_run_state = None) \/ (pre_run_state = Some("Idle")) \/ (pre_run_state = Some("Recovering")))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCancelledToAttached(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ (pre_run_state = Some("Attached"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCancelledToRetired(run_id) ==
    /\ phase = "Running"
    /\ (current_run_id = Some(run_id))
    /\ (pre_run_state = Some("Retired"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCompletedFromRetiredInFlight(run_id) ==
    /\ phase = "Retired"
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunFailedFromRetiredInFlight(run_id) ==
    /\ phase = "Retired"
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


RunCancelledFromRetiredInFlight(run_id) ==
    /\ phase = "Retired"
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
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


RecoverRequestedFromAttached ==
    /\ phase = "Attached"
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ pre_run_state' = Some("Attached")
    /\ UNCHANGED << current_run_id, wake_pending, process_pending >>


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


RetireRequestedFromAttached ==
    /\ phase = "Attached"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


ResetRequested ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Recovering" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


StopRequested ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Recovering" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ UNCHANGED << wake_pending, process_pending >>


DestroyRequested ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Recovering" \/ phase = "Retired" \/ phase = "Stopped"
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


SubmitWorkFromAttached(work_id, content_shape, handling_mode, request_id, reservation_key) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
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


AdmissionAcceptedAttachedQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect) ==
    /\ phase = "Attached"
    /\ (handling_mode = "Queue")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ wake_pending' = TRUE
    /\ process_pending' = FALSE
    /\ UNCHANGED << current_run_id, pre_run_state >>


AdmissionAcceptedAttachedSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect) ==
    /\ phase = "Attached"
    /\ (handling_mode = "Steer")
    /\ phase' = "Attached"
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


AdmissionRejectedAttached(work_id, reason) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
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


AdmissionDeduplicatedAttached(work_id, existing_work_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
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


ExternalToolDeltaReceivedAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << current_run_id, pre_run_state, wake_pending, process_pending >>


RecycleRequestedFromRetired ==
    /\ phase = "Retired"
    /\ (current_run_id = None)
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ pre_run_state' = Some("Retired")
    /\ UNCHANGED << current_run_id, wake_pending, process_pending >>


RecycleRequestedFromIdle ==
    /\ phase = "Idle"
    /\ (current_run_id = None)
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ pre_run_state' = Some("Idle")
    /\ UNCHANGED << current_run_id, wake_pending, process_pending >>


RecycleRequestedFromAttached ==
    /\ phase = "Attached"
    /\ (current_run_id = None)
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ pre_run_state' = Some("Attached")
    /\ UNCHANGED << current_run_id, wake_pending, process_pending >>


RecycleSucceeded ==
    /\ phase = "Recovering"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_state' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE


Next ==
    \/ Initialize
    \/ AttachFromIdle
    \/ DetachToIdle
    \/ \E run_id \in RunIdValues : BeginRunFromIdle(run_id)
    \/ \E run_id \in RunIdValues : BeginRunFromRetired(run_id)
    \/ \E run_id \in RunIdValues : BeginRunFromAttached(run_id)
    \/ \E run_id \in RunIdValues : BeginRunFromRecovering(run_id)
    \/ \E run_id \in RunIdValues : RunCompletedToIdle(run_id)
    \/ \E run_id \in RunIdValues : RunCompletedToAttached(run_id)
    \/ \E run_id \in RunIdValues : RunCompletedToRetired(run_id)
    \/ \E run_id \in RunIdValues : RunFailedToIdle(run_id)
    \/ \E run_id \in RunIdValues : RunFailedToAttached(run_id)
    \/ \E run_id \in RunIdValues : RunFailedToRetired(run_id)
    \/ \E run_id \in RunIdValues : RunCancelledToIdle(run_id)
    \/ \E run_id \in RunIdValues : RunCancelledToAttached(run_id)
    \/ \E run_id \in RunIdValues : RunCancelledToRetired(run_id)
    \/ \E run_id \in RunIdValues : RunCompletedFromRetiredInFlight(run_id)
    \/ \E run_id \in RunIdValues : RunFailedFromRetiredInFlight(run_id)
    \/ \E run_id \in RunIdValues : RunCancelledFromRetiredInFlight(run_id)
    \/ RecoverRequestedFromIdle
    \/ RecoverRequestedFromRunning
    \/ RecoverRequestedFromAttached
    \/ RecoverySucceeded
    \/ RetireRequestedFromIdle
    \/ RetireRequestedFromRunning
    \/ RetireRequestedFromAttached
    \/ ResetRequested
    \/ StopRequested
    \/ DestroyRequested
    \/ ResumeRequested
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : SubmitWorkFromIdle(work_id, content_shape, handling_mode, request_id, reservation_key)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : SubmitWorkFromRunning(work_id, content_shape, handling_mode, request_id, reservation_key)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : SubmitWorkFromAttached(work_id, content_shape, handling_mode, request_id, reservation_key)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedIdleQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedIdleSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedRunningQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedRunningSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedAttachedQueue(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E content_shape \in ContentShapeValues : \E handling_mode \in HandlingModeValues : \E request_id \in OptionRequestIdValues : \E reservation_key \in OptionReservationKeyValues : \E admission_effect \in AdmissionEffectValues : AdmissionAcceptedAttachedSteer(work_id, content_shape, handling_mode, request_id, reservation_key, admission_effect)
    \/ \E work_id \in WorkIdValues : \E reason \in {"alpha", "beta"} : AdmissionRejectedIdle(work_id, reason)
    \/ \E work_id \in WorkIdValues : \E reason \in {"alpha", "beta"} : AdmissionRejectedRunning(work_id, reason)
    \/ \E work_id \in WorkIdValues : \E reason \in {"alpha", "beta"} : AdmissionRejectedAttached(work_id, reason)
    \/ \E work_id \in WorkIdValues : \E existing_work_id \in WorkIdValues : AdmissionDeduplicatedIdle(work_id, existing_work_id)
    \/ \E work_id \in WorkIdValues : \E existing_work_id \in WorkIdValues : AdmissionDeduplicatedRunning(work_id, existing_work_id)
    \/ \E work_id \in WorkIdValues : \E existing_work_id \in WorkIdValues : AdmissionDeduplicatedAttached(work_id, existing_work_id)
    \/ ExternalToolDeltaReceivedIdle
    \/ ExternalToolDeltaReceivedRunning
    \/ ExternalToolDeltaReceivedRecovering
    \/ ExternalToolDeltaReceivedRetired
    \/ ExternalToolDeltaReceivedAttached
    \/ RecycleRequestedFromRetired
    \/ RecycleRequestedFromIdle
    \/ RecycleRequestedFromAttached
    \/ RecycleSucceeded
    \/ TerminalStutter

running_implies_active_run == ((phase # "Running") \/ (current_run_id # None))
active_run_only_while_running_or_retired == ((current_run_id = None) \/ (phase = "Running") \/ (phase = "Retired"))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []running_implies_active_run
THEOREM Spec => []active_run_only_while_running_or_retired

=============================================================================
