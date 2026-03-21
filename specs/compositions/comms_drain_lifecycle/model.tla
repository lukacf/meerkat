---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for comms_drain_lifecycle.

CONSTANTS CommsDrainModeValues, DrainExitReasonValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"comms_drain", "CommsDrainLifecycleMachine", "drain_plane">>
}

RouteNames == {
}

Actors == {
    "drain_plane"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "comms_drain" -> "drain_plane"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    "unknown_machine"

RouteEffect(route_name) ==
    "unknown_effect"

RouteTargetMachine(route_name) ==
    "unknown_machine"

RouteTargetInput(route_name) ==
    "unknown_input"

RouteDeliveryKind(route_name) ==
    "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ comms_drain_phase = "Inactive"
    /\ comms_drain_mode = None
    /\ comms_drain_suppresses_turn_boundary_drain = FALSE
    /\ obligation_comms_drain_spawn = {}
    /\ obligation_comms_drain_abort = {}
    /\ model_step_count = 0
    /\ pending_routes = <<>>
    /\ delivered_routes = {}
    /\ emitted_effects = {}
    /\ observed_transitions = {}

Init ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_spawn_run_stop ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "PersistentHost"], source_kind |-> "entry", source_route |-> "witness:spawn_run_stop:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "PersistentHost"], source_kind |-> "entry", source_route |-> "witness:spawn_run_stop:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "PersistentHost"], source_kind |-> "entry", source_route |-> "witness:spawn_run_stop:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "comms_drain", variant |-> "TaskSpawned", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:spawn_run_stop:2", source_machine |-> "external_entry", source_effect |-> "TaskSpawned", effect_id |-> 0], [machine |-> "comms_drain", variant |-> "StopRequested", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:spawn_run_stop:3", source_machine |-> "external_entry", source_effect |-> "StopRequested", effect_id |-> 0]>>

WitnessInit_failure_respawn ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "PersistentHost"], source_kind |-> "entry", source_route |-> "witness:failure_respawn:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "PersistentHost"], source_kind |-> "entry", source_route |-> "witness:failure_respawn:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "PersistentHost"], source_kind |-> "entry", source_route |-> "witness:failure_respawn:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "comms_drain", variant |-> "TaskExited", payload |-> [reason |-> "Failed"], source_kind |-> "entry", source_route |-> "witness:failure_respawn:2", source_machine |-> "external_entry", source_effect |-> "TaskExited", effect_id |-> 0], [machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "PersistentHost"], source_kind |-> "entry", source_route |-> "witness:failure_respawn:3", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]>>

WitnessInit_abort_observed ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "Timed"], source_kind |-> "entry", source_route |-> "witness:abort_observed:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "Timed"], source_kind |-> "entry", source_route |-> "witness:abort_observed:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> "Timed"], source_kind |-> "entry", source_route |-> "witness:abort_observed:1", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "comms_drain", variant |-> "TaskSpawned", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:abort_observed:2", source_machine |-> "external_entry", source_effect |-> "TaskSpawned", effect_id |-> 0], [machine |-> "comms_drain", variant |-> "AbortObserved", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "witness:abort_observed:3", source_machine |-> "external_entry", source_effect |-> "AbortObserved", effect_id |-> 0]>>

comms_drain_EnsureRunningFromInactive(arg_mode) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "EnsureRunning"
       /\ packet.payload.mode = arg_mode
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Inactive"
       /\ comms_drain_phase' = "Starting"
       /\ comms_drain_mode' = Some(packet.payload.mode)
       /\ comms_drain_suppresses_turn_boundary_drain' = TRUE
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SpawnDrainTask", payload |-> [mode |-> packet.payload.mode], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureRunningFromInactive"], [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureRunningFromInactive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "EnsureRunningFromInactive", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Starting"]}
       /\ obligation_comms_drain_spawn' = obligation_comms_drain_spawn \cup {[mode |-> packet.payload.mode]}
       /\ UNCHANGED << obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_TaskSpawnedFromStarting ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "TaskSpawned"
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Starting"
       /\ comms_drain_phase' = "Running"
       /\ UNCHANGED << comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "TaskSpawnedFromStarting", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Running"]}
       /\ UNCHANGED << obligation_comms_drain_spawn, obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_TaskExitedFromStartingRespawnable(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "TaskExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Starting"
       /\ (packet.payload.reason = "Failed")
       /\ (comms_drain_mode = Some("PersistentHost"))
       /\ comms_drain_phase' = "ExitedRespawnable"
       /\ comms_drain_suppresses_turn_boundary_drain' = FALSE
       /\ UNCHANGED << comms_drain_mode, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "TaskExitedFromStartingRespawnable"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "TaskExitedFromStartingRespawnable", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "ExitedRespawnable"]}
       /\ UNCHANGED << obligation_comms_drain_spawn, obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_TaskExitedFromStartingStopped(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "TaskExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Starting"
       /\ ((packet.payload.reason # "Failed") \/ (comms_drain_mode # Some("PersistentHost")))
       /\ comms_drain_phase' = "Stopped"
       /\ comms_drain_suppresses_turn_boundary_drain' = FALSE
       /\ UNCHANGED << comms_drain_mode, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "TaskExitedFromStartingStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "TaskExitedFromStartingStopped", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Stopped"]}
       /\ UNCHANGED << obligation_comms_drain_spawn, obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_TaskExitedFromRunningRespawnable(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "TaskExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Running"
       /\ (packet.payload.reason = "Failed")
       /\ (comms_drain_mode = Some("PersistentHost"))
       /\ comms_drain_phase' = "ExitedRespawnable"
       /\ comms_drain_suppresses_turn_boundary_drain' = FALSE
       /\ UNCHANGED << comms_drain_mode, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "TaskExitedFromRunningRespawnable"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "TaskExitedFromRunningRespawnable", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "ExitedRespawnable"]}
       /\ UNCHANGED << obligation_comms_drain_spawn, obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_TaskExitedFromRunningStopped(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "TaskExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Running"
       /\ ((packet.payload.reason # "Failed") \/ (comms_drain_mode # Some("PersistentHost")))
       /\ comms_drain_phase' = "Stopped"
       /\ comms_drain_suppresses_turn_boundary_drain' = FALSE
       /\ UNCHANGED << comms_drain_mode, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "TaskExitedFromRunningStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "TaskExitedFromRunningStopped", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Stopped"]}
       /\ UNCHANGED << obligation_comms_drain_spawn, obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_StopRequestedFromRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "StopRequested"
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Running"
       /\ comms_drain_phase' = "Stopped"
       /\ comms_drain_suppresses_turn_boundary_drain' = FALSE
       /\ UNCHANGED << comms_drain_mode, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "AbortDrainTask", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRequestedFromRunning"], [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "StopRequestedFromRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "StopRequestedFromRunning", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Stopped"]}
       /\ obligation_comms_drain_abort' = obligation_comms_drain_abort \cup {"token"}
       /\ UNCHANGED << obligation_comms_drain_spawn >>
       /\ model_step_count' = model_step_count + 1


comms_drain_StopRequestedFromStarting ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "StopRequested"
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Starting"
       /\ comms_drain_phase' = "Stopped"
       /\ comms_drain_suppresses_turn_boundary_drain' = FALSE
       /\ UNCHANGED << comms_drain_mode, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "AbortDrainTask", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRequestedFromStarting"], [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "StopRequestedFromStarting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "StopRequestedFromStarting", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Stopped"]}
       /\ obligation_comms_drain_abort' = obligation_comms_drain_abort \cup {"token"}
       /\ UNCHANGED << obligation_comms_drain_spawn >>
       /\ model_step_count' = model_step_count + 1


comms_drain_EnsureRunningFromExitedRespawnable(arg_mode) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "EnsureRunning"
       /\ packet.payload.mode = arg_mode
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "ExitedRespawnable"
       /\ comms_drain_phase' = "Starting"
       /\ comms_drain_mode' = Some(packet.payload.mode)
       /\ comms_drain_suppresses_turn_boundary_drain' = TRUE
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SpawnDrainTask", payload |-> [mode |-> packet.payload.mode], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureRunningFromExitedRespawnable"], [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureRunningFromExitedRespawnable"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "EnsureRunningFromExitedRespawnable", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Starting"]}
       /\ obligation_comms_drain_spawn' = obligation_comms_drain_spawn \cup {[mode |-> packet.payload.mode]}
       /\ UNCHANGED << obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_StopRequestedFromExitedRespawnable ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "StopRequested"
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "ExitedRespawnable"
       /\ comms_drain_phase' = "Stopped"
       /\ UNCHANGED << comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "StopRequestedFromExitedRespawnable", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Stopped"]}
       /\ UNCHANGED << obligation_comms_drain_spawn, obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_EnsureRunningFromStopped(arg_mode) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "EnsureRunning"
       /\ packet.payload.mode = arg_mode
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Stopped"
       /\ comms_drain_phase' = "Starting"
       /\ comms_drain_mode' = Some(packet.payload.mode)
       /\ comms_drain_suppresses_turn_boundary_drain' = TRUE
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SpawnDrainTask", payload |-> [mode |-> packet.payload.mode], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureRunningFromStopped"], [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> TRUE], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureRunningFromStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "EnsureRunningFromStopped", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Starting"]}
       /\ obligation_comms_drain_spawn' = obligation_comms_drain_spawn \cup {[mode |-> packet.payload.mode]}
       /\ UNCHANGED << obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_AbortObservedFromActive ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "comms_drain"
       /\ packet.variant = "AbortObserved"
       /\ ~HigherPriorityReady("drain_plane")
       /\ comms_drain_phase = "Running" \/ comms_drain_phase = "Starting"
       /\ comms_drain_phase' = "Stopped"
       /\ comms_drain_suppresses_turn_boundary_drain' = FALSE
       /\ UNCHANGED << comms_drain_mode, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "comms_drain", variant |-> "SetTurnBoundaryDrainSuppressed", payload |-> [active |-> FALSE], effect_id |-> (model_step_count + 1), source_transition |-> "AbortObservedFromActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "comms_drain", transition |-> "AbortObservedFromActive", actor |-> "drain_plane", step |-> (model_step_count + 1), from_phase |-> comms_drain_phase, to_phase |-> "Stopped"]}
       /\ UNCHANGED << obligation_comms_drain_spawn, obligation_comms_drain_abort >>
       /\ model_step_count' = model_step_count + 1


comms_drain_active_implies_mode_set == (((comms_drain_phase # "Starting") /\ (comms_drain_phase # "Running")) \/ (comms_drain_mode # None))
comms_drain_active_implies_suppression == (((comms_drain_phase # "Starting") /\ (comms_drain_phase # "Running")) \/ (comms_drain_suppresses_turn_boundary_drain = TRUE))

Inject_drain_ensure_running(arg_mode) ==
    /\ ~([machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> arg_mode], source_kind |-> "entry", source_route |-> "drain_ensure_running", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> arg_mode], source_kind |-> "entry", source_route |-> "drain_ensure_running", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "comms_drain", variant |-> "EnsureRunning", payload |-> [mode |-> arg_mode], source_kind |-> "entry", source_route |-> "drain_ensure_running", source_machine |-> "external_entry", source_effect |-> "EnsureRunning", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_drain_task_spawned ==
    /\ ~([machine |-> "comms_drain", variant |-> "TaskSpawned", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_task_spawned", source_machine |-> "external_entry", source_effect |-> "TaskSpawned", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "comms_drain", variant |-> "TaskSpawned", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_task_spawned", source_machine |-> "external_entry", source_effect |-> "TaskSpawned", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "comms_drain", variant |-> "TaskSpawned", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_task_spawned", source_machine |-> "external_entry", source_effect |-> "TaskSpawned", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_drain_task_exited(arg_reason) ==
    /\ ~([machine |-> "comms_drain", variant |-> "TaskExited", payload |-> [reason |-> arg_reason], source_kind |-> "entry", source_route |-> "drain_task_exited", source_machine |-> "external_entry", source_effect |-> "TaskExited", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "comms_drain", variant |-> "TaskExited", payload |-> [reason |-> arg_reason], source_kind |-> "entry", source_route |-> "drain_task_exited", source_machine |-> "external_entry", source_effect |-> "TaskExited", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "comms_drain", variant |-> "TaskExited", payload |-> [reason |-> arg_reason], source_kind |-> "entry", source_route |-> "drain_task_exited", source_machine |-> "external_entry", source_effect |-> "TaskExited", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_drain_stop_requested ==
    /\ ~([machine |-> "comms_drain", variant |-> "StopRequested", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_stop_requested", source_machine |-> "external_entry", source_effect |-> "StopRequested", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "comms_drain", variant |-> "StopRequested", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_stop_requested", source_machine |-> "external_entry", source_effect |-> "StopRequested", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "comms_drain", variant |-> "StopRequested", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_stop_requested", source_machine |-> "external_entry", source_effect |-> "StopRequested", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_drain_abort_observed ==
    /\ ~([machine |-> "comms_drain", variant |-> "AbortObserved", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_abort_observed", source_machine |-> "external_entry", source_effect |-> "AbortObserved", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "comms_drain", variant |-> "AbortObserved", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_abort_observed", source_machine |-> "external_entry", source_effect |-> "AbortObserved", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "comms_drain", variant |-> "AbortObserved", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "drain_abort_observed", source_machine |-> "external_entry", source_effect |-> "AbortObserved", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_spawn_run_stop ==
    /\ witness_current_script_input # None
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_inputs) = 0
    /\ Len(pending_routes) = 0
    /\ Len(witness_remaining_script_inputs) > 0
    /\ pending_inputs' = Append(pending_inputs, Head(witness_remaining_script_inputs))
    /\ observed_inputs' = observed_inputs \cup {Head(witness_remaining_script_inputs)}
    /\ witness_current_script_input' = Head(witness_remaining_script_inputs)
    /\ witness_remaining_script_inputs' = Tail(witness_remaining_script_inputs)
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_failure_respawn ==
    /\ witness_current_script_input # None
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_inputs) = 0
    /\ Len(pending_routes) = 0
    /\ Len(witness_remaining_script_inputs) > 0
    /\ pending_inputs' = Append(pending_inputs, Head(witness_remaining_script_inputs))
    /\ observed_inputs' = observed_inputs \cup {Head(witness_remaining_script_inputs)}
    /\ witness_current_script_input' = Head(witness_remaining_script_inputs)
    /\ witness_remaining_script_inputs' = Tail(witness_remaining_script_inputs)
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_abort_observed ==
    /\ witness_current_script_input # None
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_inputs) = 0
    /\ Len(pending_routes) = 0
    /\ Len(witness_remaining_script_inputs) > 0
    /\ pending_inputs' = Append(pending_inputs, Head(witness_remaining_script_inputs))
    /\ observed_inputs' = observed_inputs \cup {Head(witness_remaining_script_inputs)}
    /\ witness_current_script_input' = Head(witness_remaining_script_inputs)
    /\ witness_remaining_script_inputs' = Tail(witness_remaining_script_inputs)
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, obligation_comms_drain_abort, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ \E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromInactive(arg_mode)
    \/ comms_drain_TaskSpawnedFromStarting
    \/ \E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingRespawnable(arg_reason)
    \/ \E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingStopped(arg_reason)
    \/ \E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningRespawnable(arg_reason)
    \/ \E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningStopped(arg_reason)
    \/ comms_drain_StopRequestedFromRunning
    \/ comms_drain_StopRequestedFromStarting
    \/ \E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromExitedRespawnable(arg_mode)
    \/ comms_drain_StopRequestedFromExitedRespawnable
    \/ \E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromStopped(arg_mode)
    \/ comms_drain_AbortObservedFromActive
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_mode \in CommsDrainModeValues : Inject_drain_ensure_running(arg_mode)
    \/ Inject_drain_task_spawned
    \/ \E arg_reason \in DrainExitReasonValues : Inject_drain_task_exited(arg_reason)
    \/ Inject_drain_stop_requested
    \/ Inject_drain_abort_observed

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_spawn_run_stop ==
    \/ CoreNext
    \/ WitnessInjectNext_spawn_run_stop

WitnessNext_failure_respawn ==
    \/ CoreNext
    \/ WitnessInjectNext_failure_respawn

WitnessNext_abort_observed ==
    \/ CoreNext
    \/ WitnessInjectNext_abort_observed


spawn_protocol_covered == TRUE
abort_protocol_covered == TRUE

NoOpenObligationsOnTerminal_comms_drain_spawn == (comms_drain_phase = "Stopped") => obligation_comms_drain_spawn = {}
NoFeedbackWithoutObligation_comms_drain_spawn == \A input_packet \in observed_inputs : (((input_packet.machine = "comms_drain" /\ input_packet.variant = "TaskSpawned") \/ (input_packet.machine = "comms_drain" /\ input_packet.variant = "TaskExited")) => (\E record \in obligation_comms_drain_spawn : (record.mode = input_packet.payload.mode)))
NoOpenObligationsOnTerminal_comms_drain_abort == (comms_drain_phase = "Stopped") => obligation_comms_drain_abort = {}
NoFeedbackWithoutObligation_comms_drain_abort == \A input_packet \in observed_inputs : (((input_packet.machine = "comms_drain" /\ input_packet.variant = "AbortObserved")) => obligation_comms_drain_abort /= {})

\* Liveness: eventual feedback under task-scheduling fairness
OwnerFeedback_comms_drain_spawn ==
    /\ obligation_comms_drain_spawn /= {}
    /\ \E token \in obligation_comms_drain_spawn :
        /\ \E fb_variant \in {1, 2} :
           /\ pending_inputs' = Append(pending_inputs, CASE fb_variant = 1 -> [machine |-> "comms_drain", variant |-> "TaskSpawned", source_kind |-> "owner", source_machine |-> "comms_drain", source_effect |-> "SpawnDrainTask", source_route |-> "none", effect_id |-> token, payload |-> [mode |-> token.mode]] [] fb_variant = 2 -> [machine |-> "comms_drain", variant |-> "TaskExited", source_kind |-> "owner", source_machine |-> "comms_drain", source_effect |-> "SpawnDrainTask", source_route |-> "none", effect_id |-> token, payload |-> [mode |-> token.mode]])
           /\ obligation_comms_drain_spawn' = obligation_comms_drain_spawn \ {token}
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_abort, model_step_count, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

OwnerFeedback_comms_drain_abort ==
    /\ obligation_comms_drain_abort /= {}
    /\ \E token \in obligation_comms_drain_abort :
        /\ pending_inputs' = Append(pending_inputs, [machine |-> "comms_drain", variant |-> "AbortObserved", source_kind |-> "owner", source_machine |-> "comms_drain", source_effect |-> "AbortDrainTask", source_route |-> "none", effect_id |-> token, payload |-> "feedback"])
        /\ obligation_comms_drain_abort' = obligation_comms_drain_abort \ {token}
    /\ UNCHANGED << comms_drain_phase, comms_drain_mode, comms_drain_suppresses_turn_boundary_drain, obligation_comms_drain_spawn, model_step_count, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

CoverageInstrumentation == TRUE

CiStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6
WitnessStateConstraint_spawn_run_stop == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 6
WitnessStateConstraint_failure_respawn == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 6
WitnessStateConstraint_abort_observed == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 6

Spec == Init /\ [][Next]_vars
WitnessSpec_spawn_run_stop == WitnessInit_spawn_run_stop /\ [] [WitnessNext_spawn_run_stop]_vars /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromInactive(arg_mode)) /\ WF_vars(comms_drain_TaskSpawnedFromStarting) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingRespawnable(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingStopped(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningRespawnable(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningStopped(arg_reason)) /\ WF_vars(comms_drain_StopRequestedFromRunning) /\ WF_vars(comms_drain_StopRequestedFromStarting) /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromExitedRespawnable(arg_mode)) /\ WF_vars(comms_drain_StopRequestedFromExitedRespawnable) /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromStopped(arg_mode)) /\ WF_vars(comms_drain_AbortObservedFromActive) /\ WF_vars(WitnessInjectNext_spawn_run_stop)
WitnessSpec_failure_respawn == WitnessInit_failure_respawn /\ [] [WitnessNext_failure_respawn]_vars /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromInactive(arg_mode)) /\ WF_vars(comms_drain_TaskSpawnedFromStarting) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingRespawnable(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingStopped(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningRespawnable(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningStopped(arg_reason)) /\ WF_vars(comms_drain_StopRequestedFromRunning) /\ WF_vars(comms_drain_StopRequestedFromStarting) /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromExitedRespawnable(arg_mode)) /\ WF_vars(comms_drain_StopRequestedFromExitedRespawnable) /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromStopped(arg_mode)) /\ WF_vars(comms_drain_AbortObservedFromActive) /\ WF_vars(WitnessInjectNext_failure_respawn)
WitnessSpec_abort_observed == WitnessInit_abort_observed /\ [] [WitnessNext_abort_observed]_vars /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromInactive(arg_mode)) /\ WF_vars(comms_drain_TaskSpawnedFromStarting) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingRespawnable(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromStartingStopped(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningRespawnable(arg_reason)) /\ WF_vars(\E arg_reason \in DrainExitReasonValues : comms_drain_TaskExitedFromRunningStopped(arg_reason)) /\ WF_vars(comms_drain_StopRequestedFromRunning) /\ WF_vars(comms_drain_StopRequestedFromStarting) /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromExitedRespawnable(arg_mode)) /\ WF_vars(comms_drain_StopRequestedFromExitedRespawnable) /\ WF_vars(\E arg_mode \in CommsDrainModeValues : comms_drain_EnsureRunningFromStopped(arg_mode)) /\ WF_vars(comms_drain_AbortObservedFromActive) /\ WF_vars(WitnessInjectNext_abort_observed)

WitnessStateObserved_spawn_run_stop_1 == <> (comms_drain_phase = "Stopped")
WitnessTransitionObserved_spawn_run_stop_comms_drain_EnsureRunningFromInactive == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "EnsureRunningFromInactive")
WitnessTransitionObserved_spawn_run_stop_comms_drain_TaskSpawnedFromStarting == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "TaskSpawnedFromStarting")
WitnessTransitionObserved_spawn_run_stop_comms_drain_StopRequestedFromRunning == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "StopRequestedFromRunning")
WitnessTransitionOrder_spawn_run_stop_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "comms_drain" /\ earlier.transition = "EnsureRunningFromInactive" /\ later.machine = "comms_drain" /\ later.transition = "TaskSpawnedFromStarting" /\ earlier.step < later.step)
WitnessTransitionOrder_spawn_run_stop_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "comms_drain" /\ earlier.transition = "TaskSpawnedFromStarting" /\ later.machine = "comms_drain" /\ later.transition = "StopRequestedFromRunning" /\ earlier.step < later.step)
WitnessStateObserved_failure_respawn_1 == <> (comms_drain_phase = "Starting")
WitnessTransitionObserved_failure_respawn_comms_drain_EnsureRunningFromInactive == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "EnsureRunningFromInactive")
WitnessTransitionObserved_failure_respawn_comms_drain_TaskExitedFromStartingRespawnable == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "TaskExitedFromStartingRespawnable")
WitnessTransitionObserved_failure_respawn_comms_drain_EnsureRunningFromExitedRespawnable == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "EnsureRunningFromExitedRespawnable")
WitnessTransitionOrder_failure_respawn_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "comms_drain" /\ earlier.transition = "EnsureRunningFromInactive" /\ later.machine = "comms_drain" /\ later.transition = "TaskExitedFromStartingRespawnable" /\ earlier.step < later.step)
WitnessTransitionOrder_failure_respawn_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "comms_drain" /\ earlier.transition = "TaskExitedFromStartingRespawnable" /\ later.machine = "comms_drain" /\ later.transition = "EnsureRunningFromExitedRespawnable" /\ earlier.step < later.step)
WitnessStateObserved_abort_observed_1 == <> (comms_drain_phase = "Stopped")
WitnessTransitionObserved_abort_observed_comms_drain_EnsureRunningFromInactive == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "EnsureRunningFromInactive")
WitnessTransitionObserved_abort_observed_comms_drain_TaskSpawnedFromStarting == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "TaskSpawnedFromStarting")
WitnessTransitionObserved_abort_observed_comms_drain_AbortObservedFromActive == <> (\E packet \in observed_transitions : /\ packet.machine = "comms_drain" /\ packet.transition = "AbortObservedFromActive")
WitnessTransitionOrder_abort_observed_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "comms_drain" /\ earlier.transition = "EnsureRunningFromInactive" /\ later.machine = "comms_drain" /\ later.transition = "TaskSpawnedFromStarting" /\ earlier.step < later.step)
WitnessTransitionOrder_abort_observed_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "comms_drain" /\ earlier.transition = "TaskSpawnedFromStarting" /\ later.machine = "comms_drain" /\ later.transition = "AbortObservedFromActive" /\ earlier.step < later.step)

THEOREM Spec => []spawn_protocol_covered
THEOREM Spec => []abort_protocol_covered
THEOREM Spec => []comms_drain_active_implies_mode_set
THEOREM Spec => []comms_drain_active_implies_suppression
THEOREM Spec => []NoOpenObligationsOnTerminal_comms_drain_spawn
THEOREM Spec => []NoFeedbackWithoutObligation_comms_drain_spawn
THEOREM Spec => []NoOpenObligationsOnTerminal_comms_drain_abort
THEOREM Spec => []NoFeedbackWithoutObligation_comms_drain_abort

=============================================================================
