---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ExternalToolSurfaceMachine.

CONSTANTS SurfaceIdValues, TurnNumberValues

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

VARIABLES phase, model_step_count, known_surfaces, visible_surfaces, base_state, pending_op, staged_op, inflight_calls, last_delta_operation, last_delta_phase

vars == << phase, model_step_count, known_surfaces, visible_surfaces, base_state, pending_op, staged_op, inflight_calls, last_delta_operation, last_delta_phase >>

IsVisible(surface_id) == (surface_id \in visible_surfaces)
LastDeltaPhase(surface_id) == (IF ~((surface_id \in DOMAIN last_delta_phase)) THEN "None" ELSE (IF surface_id \in DOMAIN last_delta_phase THEN last_delta_phase[surface_id] ELSE "None"))
LastDeltaOperation(surface_id) == (IF ~((surface_id \in DOMAIN last_delta_operation)) THEN "None" ELSE (IF surface_id \in DOMAIN last_delta_operation THEN last_delta_operation[surface_id] ELSE "None"))
InflightCallCount(surface_id) == (IF ~((surface_id \in DOMAIN inflight_calls)) THEN 0 ELSE (IF surface_id \in DOMAIN inflight_calls THEN inflight_calls[surface_id] ELSE 0))
StagedOp(surface_id) == (IF ~((surface_id \in DOMAIN staged_op)) THEN "None" ELSE (IF surface_id \in DOMAIN staged_op THEN staged_op[surface_id] ELSE "None"))
PendingOp(surface_id) == (IF ~((surface_id \in DOMAIN pending_op)) THEN "None" ELSE (IF surface_id \in DOMAIN pending_op THEN pending_op[surface_id] ELSE "None"))
SurfaceBase(surface_id) == (IF ~((surface_id \in DOMAIN base_state)) THEN "Absent" ELSE (IF surface_id \in DOMAIN base_state THEN base_state[surface_id] ELSE "None"))

Init ==
    /\ phase = "Operating"
    /\ model_step_count = 0
    /\ known_surfaces = {}
    /\ visible_surfaces = {}
    /\ base_state = [x \in {} |-> None]
    /\ pending_op = [x \in {} |-> None]
    /\ staged_op = [x \in {} |-> None]
    /\ inflight_calls = [x \in {} |-> None]
    /\ last_delta_operation = [x \in {} |-> None]
    /\ last_delta_phase = [x \in {} |-> None]

TerminalStutter ==
    /\ phase = "Shutdown"
    /\ UNCHANGED vars

StageAdd(surface_id) ==
    /\ phase = "Operating"
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ staged_op' = MapSet(staged_op, surface_id, "Add")
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, inflight_calls, last_delta_operation, last_delta_phase >>


StageRemove(surface_id) ==
    /\ phase = "Operating"
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ staged_op' = MapSet(staged_op, surface_id, "Remove")
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, inflight_calls, last_delta_operation, last_delta_phase >>


StageReload(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ staged_op' = MapSet(staged_op, surface_id, "Reload")
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, inflight_calls, last_delta_operation, last_delta_phase >>


ApplyBoundaryAdd(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Add")
    /\ ((SurfaceBase(surface_id) = "Absent") \/ (SurfaceBase(surface_id) = "Active") \/ (SurfaceBase(surface_id) = "Removed"))
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "Add")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Add")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Pending")
    /\ UNCHANGED << visible_surfaces, base_state, inflight_calls >>


ApplyBoundaryReload(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Reload")
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "Reload")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Reload")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Pending")
    /\ UNCHANGED << visible_surfaces, base_state, inflight_calls >>


ApplyBoundaryRemoveDraining(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Remove")
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \ {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Removing")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Remove")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Draining")
    /\ UNCHANGED << inflight_calls >>


ApplyBoundaryRemoveNoop(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Remove")
    /\ (SurfaceBase(surface_id) # "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ UNCHANGED << visible_surfaces, base_state, inflight_calls, last_delta_operation, last_delta_phase >>


PendingSucceededAdd(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (PendingOp(surface_id) = "Add")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \cup {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Active")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Add")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Applied")
    /\ UNCHANGED << staged_op, inflight_calls >>


PendingSucceededReload(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (PendingOp(surface_id) = "Reload")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \cup {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Active")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Reload")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Applied")
    /\ UNCHANGED << staged_op, inflight_calls >>


PendingFailedAdd(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (PendingOp(surface_id) = "Add")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Add")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Failed")
    /\ UNCHANGED << visible_surfaces, base_state, staged_op, inflight_calls >>


PendingFailedReload(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (PendingOp(surface_id) = "Reload")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Reload")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Failed")
    /\ UNCHANGED << visible_surfaces, base_state, staged_op, inflight_calls >>


CallStartedActive(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, (InflightCallCount(surface_id) + 1))
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, last_delta_operation, last_delta_phase >>


CallStartedRejectWhileRemoving(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Removing")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, inflight_calls, last_delta_operation, last_delta_phase >>


CallStartedRejectWhileUnavailable(surface_id) ==
    /\ phase = "Operating"
    /\ ((SurfaceBase(surface_id) # "Active") /\ (SurfaceBase(surface_id) # "Removing"))
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, inflight_calls, last_delta_operation, last_delta_phase >>


CallFinishedActive(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Active")
    /\ (InflightCallCount(surface_id) > 0)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, (InflightCallCount(surface_id) - 1))
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, last_delta_operation, last_delta_phase >>


CallFinishedRemoving(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Removing")
    /\ (InflightCallCount(surface_id) > 0)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, (InflightCallCount(surface_id) - 1))
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, last_delta_operation, last_delta_phase >>


FinalizeRemovalClean(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Removing")
    /\ (InflightCallCount(surface_id) = 0)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \ {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Removed")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Remove")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Applied")
    /\ UNCHANGED << staged_op, inflight_calls >>


FinalizeRemovalForced(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Removing")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \ {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Removed")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Remove")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Forced")
    /\ UNCHANGED << staged_op >>


Shutdown ==
    /\ phase = "Operating" \/ phase = "Shutdown"
    /\ phase' = "Shutdown"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = {}
    /\ visible_surfaces' = {}
    /\ base_state' = [x \in {} |-> None]
    /\ pending_op' = [x \in {} |-> None]
    /\ staged_op' = [x \in {} |-> None]
    /\ inflight_calls' = [x \in {} |-> None]
    /\ last_delta_operation' = [x \in {} |-> None]
    /\ last_delta_phase' = [x \in {} |-> None]


Next ==
    \/ \E surface_id \in SurfaceIdValues : StageAdd(surface_id)
    \/ \E surface_id \in SurfaceIdValues : StageRemove(surface_id)
    \/ \E surface_id \in SurfaceIdValues : StageReload(surface_id)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryAdd(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryReload(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryRemoveDraining(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryRemoveNoop(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : PendingSucceededAdd(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : PendingSucceededReload(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : PendingFailedAdd(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : PendingFailedReload(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : CallStartedActive(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallStartedRejectWhileRemoving(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallStartedRejectWhileUnavailable(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallFinishedActive(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallFinishedRemoving(surface_id)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : FinalizeRemovalClean(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : FinalizeRemovalForced(surface_id, applied_at_turn)
    \/ Shutdown
    \/ TerminalStutter

removing_or_removed_surfaces_are_not_visible == (\A surface_id \in known_surfaces : (((SurfaceBase(surface_id) = "Removing") /\ ~(IsVisible(surface_id))) \/ ((SurfaceBase(surface_id) = "Removed") /\ ~(IsVisible(surface_id))) \/ ((SurfaceBase(surface_id) # "Removing") /\ (SurfaceBase(surface_id) # "Removed"))))
visible_membership_matches_active_base_state == (\A surface_id \in known_surfaces : (IsVisible(surface_id) = (SurfaceBase(surface_id) = "Active")))
removing_surfaces_have_no_pending_add_or_reload == (\A surface_id \in known_surfaces : ((SurfaceBase(surface_id) # "Removing") \/ (PendingOp(surface_id) = "None")))
removed_surfaces_only_allow_pending_none_or_add == (\A surface_id \in known_surfaces : ((SurfaceBase(surface_id) # "Removed") \/ ((PendingOp(surface_id) = "None") \/ (PendingOp(surface_id) = "Add"))))
inflight_calls_only_exist_for_active_or_removing_surfaces == (\A surface_id \in known_surfaces : ((InflightCallCount(surface_id) = 0) \/ ((SurfaceBase(surface_id) = "Active") \/ (SurfaceBase(surface_id) = "Removing"))))
reload_pending_requires_active_base_state == (\A surface_id \in known_surfaces : ((PendingOp(surface_id) # "Reload") \/ (SurfaceBase(surface_id) = "Active")))
removed_surfaces_have_zero_inflight_calls == (\A surface_id \in known_surfaces : ((SurfaceBase(surface_id) # "Removed") \/ (InflightCallCount(surface_id) = 0)))
forced_delta_phase_is_always_a_remove_delta == (\A surface_id \in known_surfaces : ((LastDeltaPhase(surface_id) # "Forced") \/ (LastDeltaOperation(surface_id) = "Remove")))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(known_surfaces) <= 1 /\ Cardinality(visible_surfaces) <= 1 /\ Cardinality(DOMAIN base_state) <= 1 /\ Cardinality(DOMAIN pending_op) <= 1 /\ Cardinality(DOMAIN staged_op) <= 1 /\ Cardinality(DOMAIN inflight_calls) <= 1 /\ Cardinality(DOMAIN last_delta_operation) <= 1 /\ Cardinality(DOMAIN last_delta_phase) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(known_surfaces) <= 2 /\ Cardinality(visible_surfaces) <= 2 /\ Cardinality(DOMAIN base_state) <= 2 /\ Cardinality(DOMAIN pending_op) <= 2 /\ Cardinality(DOMAIN staged_op) <= 2 /\ Cardinality(DOMAIN inflight_calls) <= 2 /\ Cardinality(DOMAIN last_delta_operation) <= 2 /\ Cardinality(DOMAIN last_delta_phase) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []removing_or_removed_surfaces_are_not_visible
THEOREM Spec => []visible_membership_matches_active_base_state
THEOREM Spec => []removing_surfaces_have_no_pending_add_or_reload
THEOREM Spec => []removed_surfaces_only_allow_pending_none_or_add
THEOREM Spec => []inflight_calls_only_exist_for_active_or_removing_surfaces
THEOREM Spec => []reload_pending_requires_active_base_state
THEOREM Spec => []removed_surfaces_have_zero_inflight_calls
THEOREM Spec => []forced_delta_phase_is_always_a_remove_delta

=============================================================================
