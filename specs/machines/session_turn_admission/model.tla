---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionTurnAdmissionMachine.

CONSTANTS BooleanValues, NatValues, PendingContinuationDispositionValues, RuntimeKeepAliveRequestValues, StartTurnDispatchAuthorizationValues, StartTurnDispositionValues, StartTurnExecutionKindValues, StartTurnPublicTerminalValues, TurnAdmissionPhaseValues, TurnHandlingModeValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionStartTurnPublicTerminalValues == {None} \cup {Some(x) : x \in StartTurnPublicTerminalValues}

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

VARIABLES phase, model_step_count, interrupt_pending, shutdown_pending, last_public_terminal

vars == << phase, model_step_count, interrupt_pending, shutdown_pending, last_public_terminal >>

prompt_has_content(prompt_trimmed_text_byte_count, prompt_non_text_block_count) == (IF (prompt_trimmed_text_byte_count > 0) THEN TRUE ELSE (prompt_non_text_block_count > 0))
is_active_phase(arg_phase) == (IF (arg_phase = "Admitted") THEN TRUE ELSE (IF (arg_phase = "Running") THEN TRUE ELSE (arg_phase = "Completing")))

Init ==
    /\ phase = "Idle"
    /\ model_step_count = 0
    /\ interrupt_pending = FALSE
    /\ shutdown_pending = FALSE
    /\ last_public_terminal = None

TerminalStutter ==
    /\ phase = "ShuttingDown"
    /\ UNCHANGED vars

ProjectTurnAdmissionIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ProjectTurnAdmissionAdmitted ==
    /\ phase = "Admitted"
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ProjectTurnAdmissionRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ProjectTurnAdmissionCompleting ==
    /\ phase = "Completing"
    /\ phase' = "Completing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ProjectTurnAdmissionShuttingDown ==
    /\ phase = "ShuttingDown"
    /\ phase' = "ShuttingDown"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ClaimTurn ==
    /\ phase = "Idle"
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ last_public_terminal' = None


AbortClaim ==
    /\ phase = "Admitted"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << last_public_terminal >>


BeginTurn ==
    /\ phase = "Admitted"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveTurn ==
    /\ phase = "Running"
    /\ phase' = "Completing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


FinalizeTurnToShutdown ==
    /\ phase = "Completing"
    /\ shutdown_pending
    /\ phase' = "ShuttingDown"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = FALSE
    /\ UNCHANGED << shutdown_pending, last_public_terminal >>


FinalizeTurnToIdle ==
    /\ phase = "Completing"
    /\ (shutdown_pending = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = FALSE
    /\ UNCHANGED << last_public_terminal >>


RequestInterruptAdmittedFirst ==
    /\ phase = "Admitted"
    /\ (interrupt_pending = FALSE)
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = TRUE
    /\ UNCHANGED << shutdown_pending, last_public_terminal >>


RequestInterruptAdmittedDuplicate ==
    /\ phase = "Admitted"
    /\ interrupt_pending
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


RequestInterruptRunningFirst ==
    /\ phase = "Running"
    /\ (interrupt_pending = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = TRUE
    /\ UNCHANGED << shutdown_pending, last_public_terminal >>


RequestInterruptRunningDuplicate ==
    /\ phase = "Running"
    /\ interrupt_pending
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


RequestShutdownImmediateIdle ==
    /\ phase = "Idle"
    /\ phase' = "ShuttingDown"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = TRUE
    /\ UNCHANGED << last_public_terminal >>


RequestShutdownImmediateAdmitted ==
    /\ phase = "Admitted"
    /\ phase' = "ShuttingDown"
    /\ model_step_count' = model_step_count + 1
    /\ interrupt_pending' = FALSE
    /\ shutdown_pending' = TRUE
    /\ UNCHANGED << last_public_terminal >>


RequestShutdownDeferredRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ shutdown_pending' = TRUE
    /\ UNCHANGED << interrupt_pending, last_public_terminal >>


RequestShutdownDeferredCompleting ==
    /\ phase = "Completing"
    /\ phase' = "Completing"
    /\ model_step_count' = model_step_count + 1
    /\ shutdown_pending' = TRUE
    /\ UNCHANGED << interrupt_pending, last_public_terminal >>


RequestShutdownAlreadyShuttingDown ==
    /\ phase = "ShuttingDown"
    /\ phase' = "ShuttingDown"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


AuthorizeCancelAfterBoundaryAdmitted ==
    /\ phase = "Admitted"
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


AuthorizeStartTurnDispatchAdmitted ==
    /\ phase = "Admitted"
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


AuthorizeStartTurnDispatchShuttingDown ==
    /\ phase = "ShuttingDown"
    /\ phase' = "ShuttingDown"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


AuthorizeCancelAfterBoundaryRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveDispositionContentTurn(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation) ==
    /\ phase = "Admitted"
    /\ (execution_kind_present /\ (execution_kind = "ContentTurn"))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = None
    /\ UNCHANGED << interrupt_pending, shutdown_pending >>


ResolveDispositionResumePendingWithBoundary(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation) ==
    /\ phase = "Admitted"
    /\ (execution_kind_present /\ (execution_kind = "ResumePending") /\ (pending_continuation = "RunPending"))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = None
    /\ UNCHANGED << interrupt_pending, shutdown_pending >>


ResolveDispositionResumePendingWithoutBoundary(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation) ==
    /\ phase = "Admitted"
    /\ (execution_kind_present /\ (execution_kind = "ResumePending") /\ (pending_continuation = "NoPendingBoundary"))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = Some("NoPendingBoundary")
    /\ UNCHANGED << interrupt_pending, shutdown_pending >>


ResolveDispositionDirectPrompt(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation) ==
    /\ phase = "Admitted"
    /\ ((execution_kind_present = FALSE) /\ prompt_has_content(prompt_trimmed_text_byte_count, prompt_non_text_block_count))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = None
    /\ UNCHANGED << interrupt_pending, shutdown_pending >>


ResolveDispositionDirectPending(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation) ==
    /\ phase = "Admitted"
    /\ ((execution_kind_present = FALSE) /\ (prompt_has_content(prompt_trimmed_text_byte_count, prompt_non_text_block_count) = FALSE) /\ (pending_continuation = "RunPending"))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = None
    /\ UNCHANGED << interrupt_pending, shutdown_pending >>


ResolveDispositionDirectNoPending(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation) ==
    /\ phase = "Admitted"
    /\ ((execution_kind_present = FALSE) /\ (prompt_has_content(prompt_trimmed_text_byte_count, prompt_non_text_block_count) = FALSE) /\ (pending_continuation = "NoPendingBoundary"))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = Some("NoPendingBoundary")
    /\ UNCHANGED << interrupt_pending, shutdown_pending >>


ResolveRuntimeKeepAliveEnable(keep_alive_request) ==
    /\ phase = "Admitted"
    /\ (keep_alive_request = "Enable")
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveRuntimeKeepAliveDisable(keep_alive_request) ==
    /\ phase = "Admitted"
    /\ (keep_alive_request = "Disable")
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveRuntimeKeepAlivePreserve(keep_alive_request) ==
    /\ phase = "Admitted"
    /\ (keep_alive_request = "Preserve")
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveLiveInterruptRequiredSteer(handling_mode) ==
    /\ phase = "Admitted"
    /\ (handling_mode = "Steer")
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveLiveInterruptRequiredQueue(handling_mode) ==
    /\ phase = "Admitted"
    /\ (handling_mode = "Queue")
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveLastStartTurnPublicTerminalNoPendingIdle ==
    /\ phase = "Idle"
    /\ (last_public_terminal = Some("NoPendingBoundary"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveLastStartTurnPublicTerminalNoPendingAdmitted ==
    /\ phase = "Admitted"
    /\ (last_public_terminal = Some("NoPendingBoundary"))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveLastStartTurnPublicTerminalNoPendingRunning ==
    /\ phase = "Running"
    /\ (last_public_terminal = Some("NoPendingBoundary"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveLastStartTurnPublicTerminalNoPendingCompleting ==
    /\ phase = "Completing"
    /\ (last_public_terminal = Some("NoPendingBoundary"))
    /\ phase' = "Completing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


ResolveLastStartTurnPublicTerminalNoPendingShuttingDown ==
    /\ phase = "ShuttingDown"
    /\ (last_public_terminal = Some("NoPendingBoundary"))
    /\ phase' = "ShuttingDown"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << interrupt_pending, shutdown_pending, last_public_terminal >>


Next ==
    \/ ProjectTurnAdmissionIdle
    \/ ProjectTurnAdmissionAdmitted
    \/ ProjectTurnAdmissionRunning
    \/ ProjectTurnAdmissionCompleting
    \/ ProjectTurnAdmissionShuttingDown
    \/ ClaimTurn
    \/ AbortClaim
    \/ BeginTurn
    \/ ResolveTurn
    \/ FinalizeTurnToShutdown
    \/ FinalizeTurnToIdle
    \/ RequestInterruptAdmittedFirst
    \/ RequestInterruptAdmittedDuplicate
    \/ RequestInterruptRunningFirst
    \/ RequestInterruptRunningDuplicate
    \/ RequestShutdownImmediateIdle
    \/ RequestShutdownImmediateAdmitted
    \/ RequestShutdownDeferredRunning
    \/ RequestShutdownDeferredCompleting
    \/ RequestShutdownAlreadyShuttingDown
    \/ AuthorizeCancelAfterBoundaryAdmitted
    \/ AuthorizeStartTurnDispatchAdmitted
    \/ AuthorizeStartTurnDispatchShuttingDown
    \/ AuthorizeCancelAfterBoundaryRunning
    \/ \E execution_kind_present \in BOOLEAN : \E execution_kind \in StartTurnExecutionKindValues : \E prompt_trimmed_text_byte_count \in 0..2 : \E prompt_non_text_block_count \in 0..2 : \E pending_continuation \in PendingContinuationDispositionValues : ResolveDispositionContentTurn(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
    \/ \E execution_kind_present \in BOOLEAN : \E execution_kind \in StartTurnExecutionKindValues : \E prompt_trimmed_text_byte_count \in 0..2 : \E prompt_non_text_block_count \in 0..2 : \E pending_continuation \in PendingContinuationDispositionValues : ResolveDispositionResumePendingWithBoundary(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
    \/ \E execution_kind_present \in BOOLEAN : \E execution_kind \in StartTurnExecutionKindValues : \E prompt_trimmed_text_byte_count \in 0..2 : \E prompt_non_text_block_count \in 0..2 : \E pending_continuation \in PendingContinuationDispositionValues : ResolveDispositionResumePendingWithoutBoundary(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
    \/ \E execution_kind_present \in BOOLEAN : \E execution_kind \in StartTurnExecutionKindValues : \E prompt_trimmed_text_byte_count \in 0..2 : \E prompt_non_text_block_count \in 0..2 : \E pending_continuation \in PendingContinuationDispositionValues : ResolveDispositionDirectPrompt(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
    \/ \E execution_kind_present \in BOOLEAN : \E execution_kind \in StartTurnExecutionKindValues : \E prompt_trimmed_text_byte_count \in 0..2 : \E prompt_non_text_block_count \in 0..2 : \E pending_continuation \in PendingContinuationDispositionValues : ResolveDispositionDirectPending(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
    \/ \E execution_kind_present \in BOOLEAN : \E execution_kind \in StartTurnExecutionKindValues : \E prompt_trimmed_text_byte_count \in 0..2 : \E prompt_non_text_block_count \in 0..2 : \E pending_continuation \in PendingContinuationDispositionValues : ResolveDispositionDirectNoPending(execution_kind_present, execution_kind, prompt_trimmed_text_byte_count, prompt_non_text_block_count, pending_continuation)
    \/ \E keep_alive_request \in RuntimeKeepAliveRequestValues : ResolveRuntimeKeepAliveEnable(keep_alive_request)
    \/ \E keep_alive_request \in RuntimeKeepAliveRequestValues : ResolveRuntimeKeepAliveDisable(keep_alive_request)
    \/ \E keep_alive_request \in RuntimeKeepAliveRequestValues : ResolveRuntimeKeepAlivePreserve(keep_alive_request)
    \/ \E handling_mode \in TurnHandlingModeValues : ResolveLiveInterruptRequiredSteer(handling_mode)
    \/ \E handling_mode \in TurnHandlingModeValues : ResolveLiveInterruptRequiredQueue(handling_mode)
    \/ ResolveLastStartTurnPublicTerminalNoPendingIdle
    \/ ResolveLastStartTurnPublicTerminalNoPendingAdmitted
    \/ ResolveLastStartTurnPublicTerminalNoPendingRunning
    \/ ResolveLastStartTurnPublicTerminalNoPendingCompleting
    \/ ResolveLastStartTurnPublicTerminalNoPendingShuttingDown
    \/ TerminalStutter

shutdown_phase_is_not_active == (IF (phase # "ShuttingDown") THEN TRUE ELSE (is_active_phase(phase) = FALSE))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []shutdown_phase_is_not_active

=============================================================================
