---- MODULE MeerkatMachineTarget ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Experimental target-state model for the frozen MeerkatMachine.

CONSTANTS
  SessionIdValues,
  RuntimeIdValues,
  EpochIdValues,
  InputIdValues,
  RunIdValues,
  OpIdValues,
  PeerIdValues,
  PeerItemIdValues,
  SurfaceIdValues,
  WaitRequestIdValues,
  RequestIdValues,
  ReservationKeyValues,
  BoundarySequenceValues,
  MaxSteps

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]
OptionSet(S) == {None} \cup {Some(x) : x \in S}
OptionBool == {None, Some(TRUE), Some(FALSE)}

RuntimePhaseValues ==
  {"Initializing", "Idle", "Attached", "Running", "Recovering", "Retired", "Stopped", "Destroyed"}
TurnPhaseValues ==
  {"Ready", "ApplyingPrimitive", "CallingLlm", "WaitingForOps", "DrainingBoundary", "Extracting", "ErrorRecovery", "Cancelling", "Completed", "Failed", "Cancelled"}
HandlingModeValues == {"Queue", "Steer"}
InputLifecycleStateValues ==
  {"Queued", "Staged", "AppliedPendingConsumption", "Consumed", "Superseded", "Coalesced", "Abandoned", "Cancelled", "Failed"}
InputTerminalOutcomeValues ==
  {"InputConsumed", "InputSuperseded", "InputCoalesced", "InputAbandoned", "InputCancelled", "InputFailed"}
TurnPrimitiveKindValues ==
  {"ConversationRun", "ImmediateAppend", "ImmediateContext", "PeerTurn", "DetachedContinuation"}
TurnTerminalOutcomeValues == {"TurnCompleted", "TurnFailed", "TurnCancelled"}
ControlCommandValues == {"CancelCurrentRun", "InterruptYielding", "StopRuntimeExecutor"}
OperationKindValues == {"BackgroundToolOp", "PeerReadyOp", "MobMemberChildOp", "ToolMutationOp"}
OperationStatusValues == {"Pending", "Running", "Succeeded", "Failed", "Cancelled"}
OperationTerminalOutcomeValues == {"OperationSucceeded", "OperationFailed", "OperationCancelled"}
PeerItemClassValues == {"PeerWork", "PeerRequest", "PeerResponse", "PeerEvent"}
SurfaceBaseStateValues == {"Active", "Removing"}
SurfaceMutationOpValues == {"Add", "Remove", "Reload"}
DrainModeValues == {"PersistentHost", "EphemeralHost", "AttachedHost", "Disabled"}
DrainPhaseValues == {"Inactive", "Starting", "Running", "ExitedRespawnable", "Stopped"}
ContentShapeValues == {"TextOnly", "InlineImage", "MixedMedia", "PeerPayload"}

OptionSessionIdValues == OptionSet(SessionIdValues)
OptionRuntimeIdValues == OptionSet(RuntimeIdValues)
OptionEpochIdValues == OptionSet(EpochIdValues)
OptionInputIdValues == OptionSet(InputIdValues)
OptionRunIdValues == OptionSet(RunIdValues)
OptionOpIdValues == OptionSet(OpIdValues)
OptionWaitRequestIdValues == OptionSet(WaitRequestIdValues)
OptionRequestIdValues == OptionSet(RequestIdValues)
OptionReservationKeyValues == OptionSet(ReservationKeyValues)
OptionBoundarySequenceValues == OptionSet(BoundarySequenceValues)
OptionRuntimePhaseValues == OptionSet(RuntimePhaseValues)
OptionInputLifecycleStateValues == OptionSet(InputLifecycleStateValues)
OptionInputTerminalOutcomeValues == OptionSet(InputTerminalOutcomeValues)
OptionTurnTerminalOutcomeValues == OptionSet(TurnTerminalOutcomeValues)
OptionOperationStatusValues == OptionSet(OperationStatusValues)
OptionOperationKindValues == OptionSet(OperationKindValues)
OptionOperationTerminalOutcomeValues == OptionSet(OperationTerminalOutcomeValues)
OptionPeerItemClassValues == OptionSet(PeerItemClassValues)
OptionSurfaceBaseStateValues == OptionSet(SurfaceBaseStateValues)
OptionSurfaceMutationOpValues == OptionSet(SurfaceMutationOpValues)
OptionHandlingModeValues == OptionSet(HandlingModeValues)
OptionContentShapeValues == OptionSet(ContentShapeValues)
OptionDrainModeValues == OptionSet(DrainModeValues)

TerminalInputLifecycleValues == {"Consumed", "Superseded", "Coalesced", "Abandoned", "Cancelled", "Failed"}
TerminalOperationStatusValues == {"Succeeded", "Failed", "Cancelled"}
TerminalTurnPhaseValues == {"Completed", "Failed", "Cancelled"}

ZeroCursorState ==
  [ agent_applied_cursor |-> 0,
    runtime_observed_seq |-> 0,
    runtime_last_injected_seq |-> 0 ]

NoInputOptionMap == [i \in InputIdValues |-> None]
ZeroInputNatMap == [i \in InputIdValues |-> 0]
NoOpStatusMap == [o \in OpIdValues |-> None]
NoOpKindMap == [o \in OpIdValues |-> None]
ZeroOpNatMap == [o \in OpIdValues |-> 0]
FalseOpBoolMap == [o \in OpIdValues |-> FALSE]
NoOpTerminalMap == [o \in OpIdValues |-> None]
NoPeerClassMap == [p \in PeerItemIdValues |-> None]
NoPeerCorrelationMap == [p \in PeerItemIdValues |-> None]
NoPeerTrustMap == [p \in PeerItemIdValues |-> None]
NoSurfaceBaseStateMap == [s \in SurfaceIdValues |-> None]
NoSurfaceMutationMap == [s \in SurfaceIdValues |-> None]
ZeroSurfaceNatMap == [s \in SurfaceIdValues |-> 0]

NoEffects ==
  [ wake_runtime_loop |-> FALSE,
    executor_controls |-> {},
    resolved_completion_inputs |-> {},
    injected_detached_inputs |-> {},
    published_tool_snapshot |-> FALSE,
    emitted_external_tool_update |-> FALSE,
    spawn_drain_task |-> FALSE,
    abort_drain_task |-> FALSE,
    persist_recovery_snapshot |-> FALSE ]

RECURSIVE SeqRemoveOne(_, _)
SeqRemoveOne(seq, value) ==
  IF Len(seq) = 0 THEN
    <<>>
  ELSE IF Head(seq) = value THEN
    Tail(seq)
  ELSE
    <<Head(seq)>> \o SeqRemoveOne(Tail(seq), value)

SeqToSet(seq) == {seq[i] : i \in 1..Len(seq)}
AppendUnique(seq, value) == IF value \in SeqToSet(seq) THEN seq ELSE Append(seq, value)
PickOne(S) == CHOOSE x \in S : TRUE
Present(opt) == opt # None

ResumePhase(pre) ==
  IF pre = Some("Attached") THEN
    "Attached"
  ELSE IF pre = Some("Retired") THEN
    "Retired"
  ELSE
    "Idle"

VARIABLES
  binding,
  control,
  inputs,
  completion,
  turn,
  ops,
  peer,
  tools,
  drain,
  effects,
  step_count

vars ==
  << binding, control, inputs, completion, turn, ops, peer, tools, drain, effects, step_count >>

AvailableInputs == InputIdValues \ inputs.admitted
AvailablePeerItems == PeerItemIdValues \ (SeqToSet(peer.backlog) \cup peer.terminalized_items)
AvailableOpIds == OpIdValues \ ops.known_operations
AvailableSurfaces == SurfaceIdValues \ tools.known_surfaces
TerminalOperations ==
  {o \in ops.known_operations : ops.operation_status[o] \in {Some("Succeeded"), Some("Failed"), Some("Cancelled")}}
PendingOrRunningOperations ==
  {o \in ops.known_operations : ops.operation_status[o] \in {Some("Pending"), Some("Running")}}
ReplayableBacklog == peer.replayable

HasLiveBinding == binding.live = TRUE
HasActiveRun == control.current_run_id # None
RunIdsAligned ==
  /\ control.current_run_id = inputs.current_run_id
  /\ control.current_run_id = turn.active_run_id
HasQueuedWork == \/ Len(inputs.queue) > 0 \/ Len(inputs.steer_queue) > 0
HasWaitingCompletions == completion.waiting_inputs # {}
HasPendingOps == turn.pending_op_refs # {}
BoundaryDraining == turn.phase = "DrainingBoundary"
CancellationPendingAtBoundary == /\ BoundaryDraining /\ turn.cancel_after_boundary = TRUE
Quiescent == /\ ~HasActiveRun /\ ~BoundaryDraining /\ control.pending_control = {}

BindingType ==
  [ live : BOOLEAN,
    session_id : OptionSessionIdValues,
    runtime_id : OptionRuntimeIdValues,
    driver_present : BOOLEAN,
    attachment_live : BOOLEAN,
    epoch_id : OptionEpochIdValues,
    cursor_state :
      [ agent_applied_cursor : Nat,
        runtime_observed_seq : Nat,
        runtime_last_injected_seq : Nat ] ]

ControlType ==
  [ phase : RuntimePhaseValues,
    current_run_id : OptionRunIdValues,
    pre_run_phase : OptionRuntimePhaseValues,
    wake_pending : BOOLEAN,
    process_pending : BOOLEAN,
    pending_control : SUBSET ControlCommandValues ]

InputsType ==
  [ admitted : SUBSET InputIdValues,
    content_shape : [InputIdValues -> OptionContentShapeValues],
    request_id : [InputIdValues -> OptionRequestIdValues],
    reservation_key : [InputIdValues -> OptionReservationKeyValues],
    handling_mode : [InputIdValues -> OptionHandlingModeValues],
    lifecycle : [InputIdValues -> OptionInputLifecycleStateValues],
    terminal_outcome : [InputIdValues -> OptionInputTerminalOutcomeValues],
    queue : Seq(InputIdValues),
    steer_queue : Seq(InputIdValues),
    current_run_id : OptionRunIdValues,
    current_run_contributors : Seq(InputIdValues),
    last_run_id : [InputIdValues -> OptionRunIdValues],
    last_boundary_sequence : [InputIdValues -> OptionBoundarySequenceValues] ]

CompletionType ==
  [ waiting_inputs : SUBSET InputIdValues,
    waiter_count_by_input : [InputIdValues -> Nat],
    resolved_inputs : SUBSET InputIdValues ]

TurnType ==
  [ phase : TurnPhaseValues,
    active_run_id : OptionRunIdValues,
    primitive_kind : TurnPrimitiveKindValues,
    tool_calls_pending : Nat,
    pending_op_refs : SUBSET OpIdValues,
    barrier_operation_ids : SUBSET OpIdValues,
    barrier_satisfied : BOOLEAN,
    cancel_after_boundary : BOOLEAN,
    extraction_attempts : Nat,
    max_extraction_retries : Nat,
    terminal_outcome : OptionTurnTerminalOutcomeValues ]

OpsType ==
  [ known_operations : SUBSET OpIdValues,
    operation_status : [OpIdValues -> OptionOperationStatusValues],
    operation_kind : [OpIdValues -> OptionOperationKindValues],
    watcher_count : [OpIdValues -> Nat],
    progress_count : [OpIdValues -> Nat],
    peer_ready : [OpIdValues -> BOOLEAN],
    terminal_outcome : [OpIdValues -> OptionOperationTerminalOutcomeValues],
    terminal_buffered : [OpIdValues -> BOOLEAN],
    completed_order : Seq(OpIdValues),
    active_count : Nat,
    wait_active : BOOLEAN,
    wait_request_id : OptionWaitRequestIdValues,
    wait_operation_ids : SUBSET OpIdValues ]

PeerType ==
  [ auth_required : BOOLEAN,
    trusted_peers : SUBSET PeerIdValues,
    backlog : Seq(PeerItemIdValues),
    replayable : SUBSET PeerItemIdValues,
    terminalized_items : SUBSET PeerItemIdValues,
    admitted_items : SUBSET PeerItemIdValues,
    classification : [PeerItemIdValues -> OptionPeerItemClassValues],
    correlation : [PeerItemIdValues -> OptionRequestIdValues],
    trust_snapshot : [PeerItemIdValues -> OptionBool] ]

ToolsType ==
  [ known_surfaces : SUBSET SurfaceIdValues,
    visible_surfaces : SUBSET SurfaceIdValues,
    base_state : [SurfaceIdValues -> OptionSurfaceBaseStateValues],
    staged_op : [SurfaceIdValues -> OptionSurfaceMutationOpValues],
    pending_op : [SurfaceIdValues -> OptionSurfaceMutationOpValues],
    inflight_calls : [SurfaceIdValues -> Nat],
    snapshot_epoch : Nat,
    snapshot_aligned_epoch : Nat ]

DrainType ==
  [ mode : OptionDrainModeValues,
    phase : DrainPhaseValues,
    suppressed : BOOLEAN,
    respawn_required : BOOLEAN ]

EffectsType ==
  [ wake_runtime_loop : BOOLEAN,
    executor_controls : SUBSET ControlCommandValues,
    resolved_completion_inputs : SUBSET InputIdValues,
    injected_detached_inputs : SUBSET InputIdValues,
    published_tool_snapshot : BOOLEAN,
    emitted_external_tool_update : BOOLEAN,
    spawn_drain_task : BOOLEAN,
    abort_drain_task : BOOLEAN,
    persist_recovery_snapshot : BOOLEAN ]

InputIsNonTerminal(i) == inputs.terminal_outcome[i] = None
SeqEntriesAreAdmittedNonTerminal(seq) ==
  \A i \in SeqToSet(seq) : i \in inputs.admitted /\ InputIsNonTerminal(i)

TypeOK ==
  /\ binding \in BindingType
  /\ control \in ControlType
  /\ inputs \in InputsType
  /\ completion \in CompletionType
  /\ turn \in TurnType
  /\ ops \in OpsType
  /\ peer \in PeerType
  /\ tools \in ToolsType
  /\ drain \in DrainType
  /\ effects \in EffectsType
  /\ step_count \in 0..MaxSteps
  /\ SeqEntriesAreAdmittedNonTerminal(inputs.queue)
  /\ SeqEntriesAreAdmittedNonTerminal(inputs.steer_queue)
  /\ \A i \in SeqToSet(inputs.current_run_contributors) : i \in inputs.admitted
  /\ completion.resolved_inputs \subseteq inputs.admitted
  /\ completion.waiting_inputs \subseteq inputs.admitted
  /\ completion.waiting_inputs \cap completion.resolved_inputs = {}
  /\ turn.pending_op_refs \subseteq ops.known_operations
  /\ turn.barrier_operation_ids \subseteq ops.known_operations
  /\ ops.wait_operation_ids \subseteq ops.known_operations
  /\ peer.replayable \subseteq SeqToSet(peer.backlog)
  /\ peer.admitted_items \subseteq peer.terminalized_items
  /\ peer.terminalized_items \cap SeqToSet(peer.backlog) = {}
  /\ peer.terminalized_items \cap peer.replayable = {}
  /\ tools.visible_surfaces \subseteq tools.known_surfaces
  /\ \A s \in SurfaceIdValues : tools.base_state[s] # None => s \in tools.known_surfaces
  /\ ~binding.live => drain.phase = "Inactive"

Init ==
  /\ binding =
      [ live |-> FALSE,
        session_id |-> None,
        runtime_id |-> None,
        driver_present |-> FALSE,
        attachment_live |-> FALSE,
        epoch_id |-> None,
        cursor_state |-> ZeroCursorState ]
  /\ control =
      [ phase |-> "Initializing",
        current_run_id |-> None,
        pre_run_phase |-> None,
        wake_pending |-> FALSE,
        process_pending |-> FALSE,
        pending_control |-> {} ]
  /\ inputs =
      [ admitted |-> {},
        content_shape |-> NoInputOptionMap,
        request_id |-> [i \in InputIdValues |-> None],
        reservation_key |-> [i \in InputIdValues |-> None],
        handling_mode |-> [i \in InputIdValues |-> None],
        lifecycle |-> [i \in InputIdValues |-> None],
        terminal_outcome |-> [i \in InputIdValues |-> None],
        queue |-> <<>>,
        steer_queue |-> <<>>,
        current_run_id |-> None,
        current_run_contributors |-> <<>>,
        last_run_id |-> [i \in InputIdValues |-> None],
        last_boundary_sequence |-> [i \in InputIdValues |-> None] ]
  /\ completion =
      [ waiting_inputs |-> {},
        waiter_count_by_input |-> ZeroInputNatMap,
        resolved_inputs |-> {} ]
  /\ turn =
      [ phase |-> "Ready",
        active_run_id |-> None,
        primitive_kind |-> "ConversationRun",
        tool_calls_pending |-> 0,
        pending_op_refs |-> {},
        barrier_operation_ids |-> {},
        barrier_satisfied |-> FALSE,
        cancel_after_boundary |-> FALSE,
        extraction_attempts |-> 0,
        max_extraction_retries |-> 0,
        terminal_outcome |-> None ]
  /\ ops =
      [ known_operations |-> {},
        operation_status |-> NoOpStatusMap,
        operation_kind |-> NoOpKindMap,
        watcher_count |-> ZeroOpNatMap,
        progress_count |-> ZeroOpNatMap,
        peer_ready |-> FalseOpBoolMap,
        terminal_outcome |-> NoOpTerminalMap,
        terminal_buffered |-> FalseOpBoolMap,
        completed_order |-> <<>>,
        active_count |-> 0,
        wait_active |-> FALSE,
        wait_request_id |-> None,
        wait_operation_ids |-> {} ]
  /\ peer =
      [ auth_required |-> FALSE,
        trusted_peers |-> {},
        backlog |-> <<>>,
        replayable |-> {},
        terminalized_items |-> {},
        admitted_items |-> {},
        classification |-> NoPeerClassMap,
        correlation |-> NoPeerCorrelationMap,
        trust_snapshot |-> NoPeerTrustMap ]
  /\ tools =
      [ known_surfaces |-> {},
        visible_surfaces |-> {},
        base_state |-> NoSurfaceBaseStateMap,
        staged_op |-> NoSurfaceMutationMap,
        pending_op |-> NoSurfaceMutationMap,
        inflight_calls |-> ZeroSurfaceNatMap,
        snapshot_epoch |-> 0,
        snapshot_aligned_epoch |-> 0 ]
  /\ drain =
      [ mode |-> None,
        phase |-> "Inactive",
        suppressed |-> FALSE,
        respawn_required |-> FALSE ]
  /\ effects = NoEffects
  /\ step_count = 0

PrepareBindings ==
  /\ step_count < MaxSteps
  /\ ~binding.live
  /\ control.phase = "Initializing"
  /\ binding' =
      [ binding EXCEPT
          !.live = TRUE,
          !.session_id = Some(PickOne(SessionIdValues)),
          !.runtime_id = Some(PickOne(RuntimeIdValues)),
          !.epoch_id = Some(PickOne(EpochIdValues)),
          !.cursor_state = ZeroCursorState ]
  /\ UNCHANGED << control, inputs, completion, turn, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

RegisterSession ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase = "Initializing"
  /\ control' = [control EXCEPT !.phase = "Idle"]
  /\ UNCHANGED << binding, inputs, completion, turn, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

RegisterSessionWithExecutor ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase = "Idle"
  /\ binding' = [binding EXCEPT !.driver_present = TRUE, !.attachment_live = TRUE]
  /\ control' = [control EXCEPT !.phase = "Attached"]
  /\ UNCHANGED << inputs, completion, turn, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

AdmitQueuedInput ==
  /\ step_count < MaxSteps
  /\ control.phase \notin {"Retired", "Stopped", "Destroyed"}
  /\ AvailableInputs # {}
  /\ LET i == PickOne(AvailableInputs) IN
     /\ inputs' =
         [ inputs EXCEPT
             !.admitted = @ \cup {i},
             !.content_shape[i] = Some("TextOnly"),
             !.request_id[i] = Some(PickOne(RequestIdValues)),
             !.reservation_key[i] = Some(PickOne(ReservationKeyValues)),
             !.handling_mode[i] = Some("Queue"),
             !.lifecycle[i] = Some("Queued"),
             !.terminal_outcome[i] = None,
             !.queue = Append(@, i) ]
     /\ completion' =
         [ completion EXCEPT
             !.waiting_inputs = @ \cup {i},
             !.waiter_count_by_input[i] = 1 ]
     /\ control' = [control EXCEPT !.wake_pending = TRUE, !.process_pending = FALSE]
  /\ UNCHANGED << binding, turn, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.wake_runtime_loop = TRUE, !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

AdmitSteeredInput ==
  /\ step_count < MaxSteps
  /\ control.phase \notin {"Stopped", "Destroyed"}
  /\ AvailableInputs # {}
  /\ LET i == PickOne(AvailableInputs) IN
     /\ inputs' =
         [ inputs EXCEPT
             !.admitted = @ \cup {i},
             !.content_shape[i] = Some("TextOnly"),
             !.request_id[i] = Some(PickOne(RequestIdValues)),
             !.reservation_key[i] = Some(PickOne(ReservationKeyValues)),
             !.handling_mode[i] = Some("Steer"),
             !.lifecycle[i] = Some("Queued"),
             !.terminal_outcome[i] = None,
             !.steer_queue = Append(@, i) ]
     /\ completion' =
         [ completion EXCEPT
             !.waiting_inputs = @ \cup {i},
             !.waiter_count_by_input[i] = 1 ]
     /\ control' = [control EXCEPT !.wake_pending = TRUE, !.process_pending = TRUE]
  /\ UNCHANGED << binding, turn, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.wake_runtime_loop = TRUE, !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

BeginRun ==
  /\ step_count < MaxSteps
  /\ ~HasActiveRun
  /\ HasQueuedWork
  /\ control.phase \in {"Idle", "Attached", "Recovering", "Retired"}
  /\ LET picked ==
         IF Len(inputs.steer_queue) > 0 THEN Head(inputs.steer_queue) ELSE Head(inputs.queue)
     IN
     LET pre == control.phase IN
     LET run == PickOne(RunIdValues) IN
     /\ control' =
         [ control EXCEPT
             !.phase = "Running",
             !.current_run_id = Some(run),
             !.pre_run_phase = Some(pre),
             !.wake_pending = FALSE,
             !.process_pending = FALSE ]
     /\ inputs' =
         [ inputs EXCEPT
             !.queue = IF Len(inputs.steer_queue) > 0 THEN @ ELSE SeqRemoveOne(@, picked),
             !.steer_queue = IF Len(inputs.steer_queue) > 0 THEN SeqRemoveOne(@, picked) ELSE @,
             !.current_run_id = Some(run),
             !.current_run_contributors = <<picked>>,
             !.lifecycle[picked] = Some("Staged"),
             !.last_run_id[picked] = Some(run) ]
     /\ turn' =
         [ turn EXCEPT
             !.phase = "ApplyingPrimitive",
             !.active_run_id = Some(run),
             !.primitive_kind =
               IF inputs.handling_mode[picked] = Some("Steer") THEN "ImmediateAppend" ELSE "ConversationRun",
             !.terminal_outcome = None,
             !.barrier_satisfied = FALSE ]
  /\ UNCHANGED << binding, completion, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

InterruptCurrentRun ==
  /\ step_count < MaxSteps
  /\ HasActiveRun
  /\ control' = [control EXCEPT !.pending_control = @ \cup {"CancelCurrentRun"}]
  /\ UNCHANGED << binding, inputs, completion, turn, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.executor_controls = {"CancelCurrentRun"}]
  /\ step_count' = step_count + 1

InterruptYielding ==
  /\ step_count < MaxSteps
  /\ HasActiveRun \/ binding.attachment_live
  /\ control' = [control EXCEPT !.pending_control = @ \cup {"InterruptYielding"}]
  /\ UNCHANGED << binding, inputs, completion, turn, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.executor_controls = {"InterruptYielding"}]
  /\ step_count' = step_count + 1

CancelAfterBoundary ==
  /\ step_count < MaxSteps
  /\ HasActiveRun
  /\ turn' = [turn EXCEPT !.cancel_after_boundary = TRUE]
  /\ UNCHANGED << binding, control, inputs, completion, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

RegisterPendingOps ==
  /\ step_count < MaxSteps
  /\ HasActiveRun
  /\ turn.phase \in {"ApplyingPrimitive", "CallingLlm"}
  /\ AvailableOpIds # {}
  /\ LET op == PickOne(AvailableOpIds) IN
     /\ ops' =
         [ ops EXCEPT
             !.known_operations = @ \cup {op},
             !.operation_status[op] = Some("Pending"),
             !.operation_kind[op] = Some("BackgroundToolOp"),
             !.terminal_outcome[op] = None,
             !.terminal_buffered[op] = FALSE,
             !.active_count = @ + 1 ]
     /\ turn' =
         [ turn EXCEPT
             !.phase = "WaitingForOps",
             !.tool_calls_pending = 1,
             !.pending_op_refs = @ \cup {op},
             !.barrier_operation_ids = @ \cup {op},
             !.barrier_satisfied = FALSE ]
  /\ UNCHANGED << binding, control, inputs, completion, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

WaitAllRequested ==
  /\ step_count < MaxSteps
  /\ turn.pending_op_refs # {}
  /\ ~ops.wait_active
  /\ ops' =
      [ ops EXCEPT
          !.wait_active = TRUE,
          !.wait_request_id = Some(PickOne(WaitRequestIdValues)),
          !.wait_operation_ids = turn.pending_op_refs ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

PendingSucceeded ==
  /\ step_count < MaxSteps
  /\ PendingOrRunningOperations # {}
  /\ LET op == PickOne(PendingOrRunningOperations) IN
     /\ ops' =
         [ ops EXCEPT
             !.operation_status[op] = Some("Succeeded"),
             !.terminal_outcome[op] = Some("OperationSucceeded"),
             !.terminal_buffered[op] = TRUE,
             !.completed_order = AppendUnique(@, op),
             !.active_count = IF @ = 0 THEN 0 ELSE @ - 1 ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

PendingFailed ==
  /\ step_count < MaxSteps
  /\ PendingOrRunningOperations # {}
  /\ LET op == PickOne(PendingOrRunningOperations) IN
     /\ ops' =
         [ ops EXCEPT
             !.operation_status[op] = Some("Failed"),
             !.terminal_outcome[op] = Some("OperationFailed"),
             !.terminal_buffered[op] = TRUE,
             !.completed_order = AppendUnique(@, op),
             !.active_count = IF @ = 0 THEN 0 ELSE @ - 1 ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

WaitAllSatisfied ==
  /\ step_count < MaxSteps
  /\ ops.wait_active
  /\ ops.wait_operation_ids \subseteq TerminalOperations
  /\ ops' =
      [ ops EXCEPT
          !.wait_active = FALSE,
          !.wait_request_id = None,
          !.wait_operation_ids = {} ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

OpsBarrierSatisfied ==
  /\ step_count < MaxSteps
  /\ turn.phase = "WaitingForOps"
  /\ turn.barrier_operation_ids # {}
  /\ turn.barrier_operation_ids \subseteq TerminalOperations
  /\ turn' = [turn EXCEPT !.barrier_satisfied = TRUE]
  /\ UNCHANGED << binding, control, inputs, completion, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

ToolCallsResolved ==
  /\ step_count < MaxSteps
  /\ turn.phase = "WaitingForOps"
  /\ turn.barrier_satisfied
  /\ turn' = [turn EXCEPT !.tool_calls_pending = 0, !.phase = "DrainingBoundary"]
  /\ UNCHANGED << binding, control, inputs, completion, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

BoundaryContinue ==
  /\ step_count < MaxSteps
  /\ turn.phase = "DrainingBoundary"
  /\ ~turn.cancel_after_boundary
  /\ turn' = [turn EXCEPT !.phase = "Completed", !.terminal_outcome = Some("TurnCompleted")]
  /\ UNCHANGED << binding, control, inputs, completion, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

CancellationObserved ==
  /\ step_count < MaxSteps
  /\ CancellationPendingAtBoundary
  /\ turn' = [turn EXCEPT !.phase = "Cancelled", !.terminal_outcome = Some("TurnCancelled")]
  /\ UNCHANGED << binding, control, inputs, completion, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

BoundaryComplete ==
  /\ step_count < MaxSteps
  /\ turn.phase \in TerminalTurnPhaseValues
  /\ HasActiveRun
  /\ LET contributors == SeqToSet(inputs.current_run_contributors) IN
     /\ control' =
         [ control EXCEPT
             !.phase = ResumePhase(control.pre_run_phase),
             !.current_run_id = None,
             !.pre_run_phase = None,
             !.pending_control = {} ]
     /\ inputs' =
         [ inputs EXCEPT
             !.current_run_id = None,
             !.current_run_contributors = <<>>,
             !.lifecycle =
               [i \in InputIdValues |->
                 IF i \in contributors THEN
                   IF turn.terminal_outcome = Some("TurnCompleted") THEN Some("Consumed")
                   ELSE IF turn.terminal_outcome = Some("TurnCancelled") THEN Some("Cancelled")
                   ELSE Some("Failed")
                 ELSE inputs.lifecycle[i]],
             !.terminal_outcome =
               [i \in InputIdValues |->
                 IF i \in contributors THEN
                   IF turn.terminal_outcome = Some("TurnCompleted") THEN Some("InputConsumed")
                   ELSE IF turn.terminal_outcome = Some("TurnCancelled") THEN Some("InputCancelled")
                   ELSE Some("InputFailed")
                 ELSE inputs.terminal_outcome[i]],
             !.last_boundary_sequence =
               [i \in InputIdValues |->
                 IF i \in contributors THEN Some(PickOne(BoundarySequenceValues)) ELSE inputs.last_boundary_sequence[i]] ]
     /\ completion' =
         [ completion EXCEPT
             !.waiting_inputs = @ \ contributors,
             !.resolved_inputs = @ \cup contributors,
             !.waiter_count_by_input =
               [i \in InputIdValues |->
                 IF i \in contributors THEN 0 ELSE completion.waiter_count_by_input[i]] ]
     /\ turn' =
         [ turn EXCEPT
             !.phase = "Ready",
             !.active_run_id = None,
             !.tool_calls_pending = 0,
             !.pending_op_refs = {},
             !.barrier_operation_ids = {},
             !.barrier_satisfied = FALSE,
             !.cancel_after_boundary = FALSE,
             !.terminal_outcome = None ]
  /\ UNCHANGED << binding, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.resolved_completion_inputs = SeqToSet(inputs.current_run_contributors)]
  /\ step_count' = step_count + 1

RetryRequested ==
  /\ step_count < MaxSteps
  /\ turn.phase \in {"Failed", "Extracting", "ErrorRecovery"}
  /\ turn.extraction_attempts < turn.max_extraction_retries
  /\ turn' =
      [ turn EXCEPT
          !.phase = "ApplyingPrimitive",
          !.extraction_attempts = @ + 1,
          !.terminal_outcome = None,
          !.cancel_after_boundary = FALSE ]
  /\ UNCHANGED << binding, control, inputs, completion, ops, peer, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

ResolveCompletionWaiters ==
  /\ step_count < MaxSteps
  /\ {i \in completion.waiting_inputs : inputs.terminal_outcome[i] # None} # {}
  /\ LET i == PickOne({j \in completion.waiting_inputs : inputs.terminal_outcome[j] # None}) IN
     /\ completion' =
         [ completion EXCEPT
             !.waiting_inputs = @ \ {i},
             !.resolved_inputs = @ \cup {i},
             !.waiter_count_by_input[i] = 0 ]
     /\ UNCHANGED << binding, control, inputs, turn, ops, peer, tools, drain >>
     /\ effects' = [NoEffects EXCEPT !.resolved_completion_inputs = {i}]
     /\ step_count' = step_count + 1

InjectDetachedWakeContinuation ==
  /\ step_count < MaxSteps
  /\ Quiescent
  /\ {o \in ops.known_operations :
       ops.operation_kind[o] = Some("BackgroundToolOp") /\ ops.terminal_buffered[o]} # {}
  /\ AvailableInputs # {}
  /\ LET op == PickOne({o \in ops.known_operations :
                          ops.operation_kind[o] = Some("BackgroundToolOp") /\ ops.terminal_buffered[o]}) IN
     LET i == PickOne(AvailableInputs) IN
     /\ inputs' =
         [ inputs EXCEPT
             !.admitted = @ \cup {i},
             !.content_shape[i] = Some("PeerPayload"),
             !.handling_mode[i] = Some("Queue"),
             !.lifecycle[i] = Some("Queued"),
             !.terminal_outcome[i] = None,
             !.queue = Append(@, i) ]
     /\ ops' = [ops EXCEPT !.terminal_buffered[op] = FALSE]
     /\ control' = [control EXCEPT !.wake_pending = TRUE]
     /\ UNCHANGED << binding, completion, turn, peer, tools, drain >>
     /\ effects' = [NoEffects EXCEPT !.wake_runtime_loop = TRUE, !.injected_detached_inputs = {i}, !.persist_recovery_snapshot = TRUE]
     /\ step_count' = step_count + 1

RegisterTrustedPeer ==
  /\ step_count < MaxSteps
  /\ PeerIdValues \ peer.trusted_peers # {}
  /\ LET p == PickOne(PeerIdValues \ peer.trusted_peers) IN
     /\ peer' = [peer EXCEPT !.trusted_peers = @ \cup {p}]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

UnregisterTrustedPeer ==
  /\ step_count < MaxSteps
  /\ peer.trusted_peers # {}
  /\ LET p == PickOne(peer.trusted_peers) IN
     /\ peer' = [peer EXCEPT !.trusted_peers = @ \ {p}]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

ReceivePeerIngress ==
  /\ step_count < MaxSteps
  /\ AvailablePeerItems # {}
  /\ \E cls \in PeerItemClassValues :
      LET item == PickOne(AvailablePeerItems) IN
      /\ peer' =
          [ peer EXCEPT
              !.backlog = Append(@, item),
              !.replayable = IF cls = "PeerWork" THEN @ \cup {item} ELSE @,
              !.classification[item] = Some(cls),
              !.correlation[item] = Some(PickOne(RequestIdValues)),
              !.trust_snapshot[item] = Some(peer.trusted_peers # {}) ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

AdmitPeerIngress ==
  /\ step_count < MaxSteps
  /\ ReplayableBacklog # {}
  /\ AvailableInputs # {}
  /\ LET item == PickOne(ReplayableBacklog) IN
     LET i == PickOne(AvailableInputs) IN
     /\ inputs' =
         [ inputs EXCEPT
             !.admitted = @ \cup {i},
             !.content_shape[i] = Some("PeerPayload"),
             !.handling_mode[i] = Some("Queue"),
             !.lifecycle[i] = Some("Queued"),
             !.terminal_outcome[i] = None,
             !.queue = Append(@, i) ]
     /\ peer' =
         [ peer EXCEPT
             !.backlog = SeqRemoveOne(@, item),
             !.replayable = @ \ {item},
             !.terminalized_items = @ \cup {item},
             !.admitted_items = @ \cup {item} ]
     /\ control' = [control EXCEPT !.wake_pending = TRUE]
  /\ UNCHANGED << binding, completion, turn, ops, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.wake_runtime_loop = TRUE, !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

ReplayPeerIngress ==
  /\ step_count < MaxSteps
  /\ control.phase \in {"Recovering", "Initializing"}
  /\ ReplayableBacklog # {}
  /\ AvailableInputs # {}
  /\ LET item == PickOne(ReplayableBacklog) IN
     LET i == PickOne(AvailableInputs) IN
     /\ inputs' =
         [ inputs EXCEPT
             !.admitted = @ \cup {i},
             !.content_shape[i] = Some("PeerPayload"),
             !.handling_mode[i] = Some("Queue"),
             !.lifecycle[i] = Some("Queued"),
             !.terminal_outcome[i] = None,
             !.queue = Append(@, i) ]
     /\ peer' =
         [ peer EXCEPT
             !.backlog = SeqRemoveOne(@, item),
             !.replayable = @ \ {item},
             !.terminalized_items = @ \cup {item},
             !.admitted_items = @ \cup {item} ]
     /\ control' = [control EXCEPT !.wake_pending = TRUE]
  /\ UNCHANGED << binding, completion, turn, ops, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.wake_runtime_loop = TRUE, !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

DrainPeerIngress ==
  /\ step_count < MaxSteps
  /\ Len(peer.backlog) > 0
  /\ LET item == Head(peer.backlog) IN
     /\ peer' =
         [ peer EXCEPT
             !.backlog = Tail(@),
             !.replayable = @ \ {item},
             !.terminalized_items = @ \cup {item},
             !.classification[item] = None,
             !.correlation[item] = None,
             !.trust_snapshot[item] = None ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

UpdatePeerIngressContext ==
  /\ step_count < MaxSteps
  /\ peer' = [peer EXCEPT !.auth_required = ~@]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, tools, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

StageAdd ==
  /\ step_count < MaxSteps
  /\ AvailableSurfaces # {}
  /\ LET s == PickOne(AvailableSurfaces) IN
     /\ tools' =
         [ tools EXCEPT
             !.known_surfaces = @ \cup {s},
             !.staged_op[s] = Some("Add") ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

StageRemove ==
  /\ step_count < MaxSteps
  /\ tools.visible_surfaces # {}
  /\ LET s == PickOne(tools.visible_surfaces) IN
     /\ tools' =
         [ tools EXCEPT
             !.base_state[s] = Some("Removing"),
             !.staged_op[s] = Some("Remove") ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

StageReload ==
  /\ step_count < MaxSteps
  /\ {s \in tools.known_surfaces : tools.base_state[s] # None} # {}
  /\ LET s == PickOne({x \in tools.known_surfaces : tools.base_state[x] # None}) IN
     /\ tools' = [tools EXCEPT !.staged_op[s] = Some("Reload")]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, drain >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

ApplyBoundary ==
  /\ step_count < MaxSteps
  /\ {s \in SurfaceIdValues : tools.staged_op[s] # None} # {}
  /\ LET s == PickOne({x \in SurfaceIdValues : tools.staged_op[x] # None}) IN
     LET op == tools.staged_op[s] IN
     /\ tools' =
         [ tools EXCEPT
             !.visible_surfaces =
               IF op = Some("Add") THEN @ \cup {s}
               ELSE IF op = Some("Remove") THEN @
               ELSE @,
             !.base_state[s] =
               IF op = Some("Remove") THEN Some("Removing") ELSE Some("Active"),
             !.pending_op[s] =
               IF op = Some("Remove") THEN Some("Remove") ELSE None,
             !.staged_op[s] = None,
             !.snapshot_epoch = @ + 1,
             !.snapshot_aligned_epoch = @ + 1 ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, drain >>
  /\ effects' =
      [ NoEffects EXCEPT
          !.published_tool_snapshot = TRUE,
          !.emitted_external_tool_update = TRUE,
          !.persist_recovery_snapshot = TRUE ]
  /\ step_count' = step_count + 1

FinalizeRemovalClean ==
  /\ step_count < MaxSteps
  /\ {s \in SurfaceIdValues :
       tools.pending_op[s] = Some("Remove") /\ tools.inflight_calls[s] = 0} # {}
  /\ LET s == PickOne({x \in SurfaceIdValues :
                         tools.pending_op[x] = Some("Remove") /\ tools.inflight_calls[x] = 0}) IN
     /\ tools' =
         [ tools EXCEPT
             !.known_surfaces = @ \ {s},
             !.visible_surfaces = @ \ {s},
             !.base_state[s] = None,
             !.staged_op[s] = None,
             !.pending_op[s] = None ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

FinalizeRemovalForced ==
  /\ step_count < MaxSteps
  /\ {s \in SurfaceIdValues : tools.pending_op[s] = Some("Remove")} # {}
  /\ LET s == PickOne({x \in SurfaceIdValues : tools.pending_op[x] = Some("Remove")}) IN
     /\ tools' =
         [ tools EXCEPT
             !.known_surfaces = @ \ {s},
             !.visible_surfaces = @ \ {s},
             !.base_state[s] = None,
             !.staged_op[s] = None,
             !.pending_op[s] = None,
             !.inflight_calls[s] = 0 ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

EnsureDrainRunning ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ drain.mode # None
  /\ drain.mode # Some("Disabled")
  /\ drain.phase \in {"Inactive", "Stopped", "ExitedRespawnable"}
  /\ drain' =
      [ drain EXCEPT
          !.phase = "Starting",
          !.respawn_required = FALSE ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, tools >>
  /\ effects' = [NoEffects EXCEPT !.spawn_drain_task = TRUE]
  /\ step_count' = step_count + 1

TaskSpawned ==
  /\ step_count < MaxSteps
  /\ drain.phase = "Starting"
  /\ drain' = [drain EXCEPT !.phase = "Running"]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, tools >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

TaskExited ==
  /\ step_count < MaxSteps
  /\ drain.phase = "Running"
  /\ drain' =
      [ drain EXCEPT
          !.phase = "ExitedRespawnable",
          !.respawn_required = TRUE ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, tools >>
  /\ effects' = NoEffects
  /\ step_count' = step_count + 1

AbortDrain ==
  /\ step_count < MaxSteps
  /\ drain.phase \in {"Starting", "Running", "ExitedRespawnable"}
  /\ drain' =
      [ drain EXCEPT
          !.phase = "Stopped",
          !.suppressed = TRUE,
          !.respawn_required = FALSE ]
  /\ UNCHANGED << binding, control, inputs, completion, turn, ops, peer, tools >>
  /\ effects' = [NoEffects EXCEPT !.abort_drain_task = TRUE]
  /\ step_count' = step_count + 1

RetireRuntime ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase \notin {"Destroyed", "Stopped"}
  /\ control' =
      [ control EXCEPT
          !.phase = "Retired",
          !.wake_pending = FALSE,
          !.process_pending = FALSE ]
  /\ completion' =
      IF ~HasActiveRun /\ ~HasQueuedWork THEN
        [ completion EXCEPT
            !.waiting_inputs = {},
            !.waiter_count_by_input =
              [i \in InputIdValues |->
                IF i \in completion.waiting_inputs THEN 0 ELSE completion.waiter_count_by_input[i]] ]
      ELSE completion
  /\ UNCHANGED << binding, inputs, turn, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

StopRuntime ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase \notin {"Destroyed", "Stopped"}
  /\ control' =
      [ control EXCEPT
          !.phase = "Stopped",
          !.current_run_id = None,
          !.pre_run_phase = None,
          !.wake_pending = FALSE,
          !.process_pending = FALSE,
          !.pending_control = {} ]
  /\ inputs' =
      [ inputs EXCEPT
          !.queue = <<>>,
          !.steer_queue = <<>>,
          !.current_run_id = None,
          !.current_run_contributors = <<>> ]
  /\ completion' =
      [ completion EXCEPT
          !.waiting_inputs = {},
          !.waiter_count_by_input =
            [i \in InputIdValues |->
              IF i \in completion.waiting_inputs THEN 0 ELSE completion.waiter_count_by_input[i]] ]
  /\ turn' =
      [ turn EXCEPT
          !.phase = "Ready",
          !.active_run_id = None,
          !.pending_op_refs = {},
          !.barrier_operation_ids = {},
          !.barrier_satisfied = FALSE,
          !.cancel_after_boundary = FALSE,
          !.terminal_outcome = None ]
  /\ UNCHANGED << binding, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.executor_controls = {"StopRuntimeExecutor"}, !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

ResetRuntime ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase # "Destroyed"
  /\ {e \in EpochIdValues : Some(e) # binding.epoch_id} # {}
  /\ binding' = [binding EXCEPT !.epoch_id = Some(PickOne({e \in EpochIdValues : Some(e) # binding.epoch_id}))]
  /\ control' =
      [ control EXCEPT
          !.phase = IF binding.attachment_live THEN "Attached" ELSE "Idle",
          !.current_run_id = None,
          !.pre_run_phase = None,
          !.wake_pending = FALSE,
          !.process_pending = FALSE,
          !.pending_control = {} ]
  /\ inputs' =
      [ inputs EXCEPT
          !.queue = <<>>,
          !.steer_queue = <<>>,
          !.current_run_id = None,
          !.current_run_contributors = <<>> ]
  /\ completion' =
      [ completion EXCEPT
          !.waiting_inputs = {},
          !.waiter_count_by_input =
            [i \in InputIdValues |->
              IF i \in completion.waiting_inputs THEN 0 ELSE completion.waiter_count_by_input[i]] ]
  /\ turn' =
      [ turn EXCEPT
          !.phase = "Ready",
          !.active_run_id = None,
          !.pending_op_refs = {},
          !.barrier_operation_ids = {},
          !.barrier_satisfied = FALSE,
          !.cancel_after_boundary = FALSE,
          !.terminal_outcome = None ]
  /\ peer' = peer
  /\ tools' = tools
  /\ drain' = drain
  /\ UNCHANGED ops
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

RecoverRuntime ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase \notin {"Destroyed"}
  /\ control' =
      [ control EXCEPT
          !.phase =
            IF HasQueuedWork THEN "Recovering"
            ELSE IF binding.attachment_live THEN "Attached" ELSE "Idle",
          !.wake_pending = HasQueuedWork,
          !.process_pending = Len(inputs.steer_queue) > 0,
          !.current_run_id = None,
          !.pre_run_phase = None,
          !.pending_control = {} ]
  /\ inputs' = [inputs EXCEPT !.current_run_id = None, !.current_run_contributors = <<>>]
  /\ turn' = [turn EXCEPT !.active_run_id = None]
  /\ UNCHANGED << binding, completion, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

RecycleRuntime ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase \notin {"Destroyed"}
  /\ control' =
      [ control EXCEPT
          !.phase = IF binding.attachment_live THEN "Attached" ELSE "Idle",
          !.wake_pending = HasQueuedWork,
          !.process_pending = Len(inputs.steer_queue) > 0,
          !.current_run_id = None,
          !.pre_run_phase = None,
          !.pending_control = {} ]
  /\ inputs' = [inputs EXCEPT !.current_run_id = None, !.current_run_contributors = <<>>]
  /\ turn' = [turn EXCEPT !.active_run_id = None]
  /\ UNCHANGED << binding, completion, ops, peer, tools, drain >>
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

DestroyRuntime ==
  /\ step_count < MaxSteps
  /\ binding.live
  /\ control.phase # "Destroyed"
  /\ binding' =
      [ binding EXCEPT
          !.live = FALSE,
          !.session_id = None,
          !.runtime_id = None,
          !.driver_present = FALSE,
          !.attachment_live = FALSE,
          !.epoch_id = None,
          !.cursor_state = ZeroCursorState ]
  /\ control' =
      [ control EXCEPT
          !.phase = "Destroyed",
          !.current_run_id = None,
          !.pre_run_phase = None,
          !.wake_pending = FALSE,
          !.process_pending = FALSE,
          !.pending_control = {} ]
  /\ inputs' =
      [ inputs EXCEPT
          !.admitted = {},
          !.content_shape = NoInputOptionMap,
          !.request_id = [i \in InputIdValues |-> None],
          !.reservation_key = [i \in InputIdValues |-> None],
          !.handling_mode = [i \in InputIdValues |-> None],
          !.lifecycle = [i \in InputIdValues |-> None],
          !.terminal_outcome = [i \in InputIdValues |-> None],
          !.queue = <<>>,
          !.steer_queue = <<>>,
          !.current_run_id = None,
          !.current_run_contributors = <<>>,
          !.last_run_id = [i \in InputIdValues |-> None],
          !.last_boundary_sequence = [i \in InputIdValues |-> None] ]
  /\ completion' =
      [ completion EXCEPT
          !.waiting_inputs = {},
          !.waiter_count_by_input = ZeroInputNatMap,
          !.resolved_inputs = {} ]
  /\ turn' =
      [ turn EXCEPT
          !.phase = "Ready",
          !.active_run_id = None,
          !.tool_calls_pending = 0,
          !.pending_op_refs = {},
          !.barrier_operation_ids = {},
          !.barrier_satisfied = FALSE,
          !.cancel_after_boundary = FALSE,
          !.extraction_attempts = 0,
          !.terminal_outcome = None ]
  /\ ops' =
      [ ops EXCEPT
          !.wait_active = FALSE,
          !.wait_request_id = None,
          !.wait_operation_ids = {} ]
  /\ peer' =
      [ peer EXCEPT
          !.trusted_peers = {},
          !.backlog = <<>>,
          !.replayable = {},
          !.terminalized_items = {},
          !.admitted_items = {},
          !.classification = NoPeerClassMap,
          !.correlation = NoPeerCorrelationMap,
          !.trust_snapshot = NoPeerTrustMap ]
  /\ tools' =
      [ tools EXCEPT
          !.known_surfaces = {},
          !.visible_surfaces = {},
          !.base_state = NoSurfaceBaseStateMap,
          !.staged_op = NoSurfaceMutationMap,
          !.pending_op = NoSurfaceMutationMap,
          !.inflight_calls = ZeroSurfaceNatMap ]
  /\ drain' =
      [ drain EXCEPT
          !.mode = None,
          !.phase = "Inactive",
          !.suppressed = FALSE,
          !.respawn_required = FALSE ]
  /\ effects' = [NoEffects EXCEPT !.persist_recovery_snapshot = TRUE]
  /\ step_count' = step_count + 1

Stutter ==
  /\ UNCHANGED vars

MachineAction ==
  \/ PrepareBindings
  \/ RegisterSession
  \/ RegisterSessionWithExecutor
  \/ AdmitQueuedInput
  \/ AdmitSteeredInput
  \/ BeginRun
  \/ InterruptCurrentRun
  \/ InterruptYielding
  \/ CancelAfterBoundary
  \/ RegisterPendingOps
  \/ WaitAllRequested
  \/ PendingSucceeded
  \/ PendingFailed
  \/ WaitAllSatisfied
  \/ OpsBarrierSatisfied
  \/ ToolCallsResolved
  \/ BoundaryContinue
  \/ CancellationObserved
  \/ BoundaryComplete
  \/ RetryRequested
  \/ ResolveCompletionWaiters
  \/ InjectDetachedWakeContinuation
  \/ RegisterTrustedPeer
  \/ UnregisterTrustedPeer
  \/ ReceivePeerIngress
  \/ AdmitPeerIngress
  \/ ReplayPeerIngress
  \/ DrainPeerIngress
  \/ UpdatePeerIngressContext
  \/ StageAdd
  \/ StageRemove
  \/ StageReload
  \/ ApplyBoundary
  \/ FinalizeRemovalClean
  \/ FinalizeRemovalForced
  \/ EnsureDrainRunning
  \/ TaskSpawned
  \/ TaskExited
  \/ AbortDrain
  \/ RetireRuntime
  \/ StopRuntime
  \/ ResetRuntime
  \/ RecoverRuntime
  \/ RecycleRuntime
  \/ DestroyRuntime

Next ==
  \/ /\ control.phase # "Destroyed"
     /\ MachineAction
  \/ Stutter

RunIdsAlignedInvariant == ~HasActiveRun \/ RunIdsAligned
RunningHasActiveRunInvariant == control.phase = "Running" => HasActiveRun
ActiveRunPhaseInvariant == HasActiveRun => control.phase \in {"Running", "Retired"}
LiveBindingLifecycleInvariant ==
  control.phase \notin {"Initializing", "Destroyed"} => binding.live
WaitingInputsInvariant ==
  \A i \in completion.waiting_inputs :
    /\ i \in inputs.admitted
    /\ inputs.terminal_outcome[i] = None
    /\ completion.waiter_count_by_input[i] > 0
ResolvedInputsInvariant == completion.resolved_inputs \subseteq inputs.admitted
ResolvedInputsClearedInvariant ==
  \A i \in completion.resolved_inputs :
    /\ i \notin completion.waiting_inputs
    /\ completion.waiter_count_by_input[i] = 0
QueueSteerDisjointInvariant == SeqToSet(inputs.queue) \cap SeqToSet(inputs.steer_queue) = {}
QueueHandlingInvariant ==
  \A i \in SeqToSet(inputs.queue) :
    /\ inputs.handling_mode[i] = Some("Queue")
    /\ inputs.lifecycle[i] = Some("Queued")
SteerHandlingInvariant ==
  \A i \in SeqToSet(inputs.steer_queue) :
    /\ inputs.handling_mode[i] = Some("Steer")
    /\ inputs.lifecycle[i] = Some("Queued")
ContributorLifecycleInvariant ==
  \A i \in SeqToSet(inputs.current_run_contributors) : inputs.lifecycle[i] = Some("Staged")
TerminalInputsNotQueuedInvariant ==
  \A i \in inputs.admitted :
    inputs.terminal_outcome[i] # None =>
      /\ i \notin SeqToSet(inputs.queue)
      /\ i \notin SeqToSet(inputs.steer_queue)
CurrentRunContributorsInvariant ==
  HasActiveRun => Len(inputs.current_run_contributors) > 0
PendingOpsWhenWaitingInvariant ==
  turn.phase = "WaitingForOps" => turn.pending_op_refs # {}
BarrierSubsetInvariant == turn.barrier_operation_ids \subseteq turn.pending_op_refs
BarrierSatisfiedInvariant ==
  turn.barrier_satisfied => turn.barrier_operation_ids \subseteq TerminalOperations
ReadyTurnInvariant ==
  turn.phase = "Ready" =>
    /\ turn.active_run_id = None
    /\ turn.pending_op_refs = {}
    /\ turn.barrier_operation_ids = {}
    /\ turn.cancel_after_boundary = FALSE
    /\ turn.terminal_outcome = None
WaitAllAlignmentInvariant ==
  /\ ops.wait_active = (ops.wait_request_id # None)
  /\ ops.wait_request_id = None => ops.wait_operation_ids = {}
  /\ ops.wait_active => ops.wait_operation_ids # {}
ActiveCountInvariant == ops.active_count = Cardinality(PendingOrRunningOperations)
VisibleSurfacesInvariant == tools.visible_surfaces \subseteq tools.known_surfaces
VisibleSurfacesMatchAppliedStateInvariant ==
  tools.visible_surfaces = {s \in SurfaceIdValues : tools.base_state[s] # None}
VisibleSurfaceBaseStateInvariant ==
  \A s \in tools.visible_surfaces : tools.base_state[s] # None
StagedSurfaceKnownInvariant ==
  \A s \in SurfaceIdValues : tools.staged_op[s] # None => s \in tools.known_surfaces
PendingRemovalStateInvariant ==
  \A s \in SurfaceIdValues :
    tools.pending_op[s] = Some("Remove") => tools.base_state[s] = Some("Removing")
InflightSurfaceStateInvariant ==
  \A s \in SurfaceIdValues : tools.inflight_calls[s] > 0 => tools.base_state[s] # None
SnapshotEpochInvariant == tools.snapshot_aligned_epoch <= tools.snapshot_epoch
ReplayablePeerClassInvariant ==
  \A item \in peer.replayable : peer.classification[item] = Some("PeerWork")
PeerTerminalizedBacklogInvariant ==
  peer.terminalized_items \cap SeqToSet(peer.backlog) = {}
PeerAdmittedLineageInvariant ==
  peer.admitted_items \subseteq peer.terminalized_items
DrainBindingInvariant ==
  drain.phase # "Inactive" => binding.live
DrainModeInvariant ==
  drain.phase # "Inactive" => /\ drain.mode # None /\ drain.mode # Some("Disabled")
DestroyedShapeInvariant ==
  control.phase = "Destroyed" =>
    /\ ~binding.live
    /\ Len(inputs.queue) = 0
    /\ Len(inputs.steer_queue) = 0
    /\ completion.waiting_inputs = {}
StateConstraint == step_count <= MaxSteps

Spec == Init /\ [][Next]_vars

====
