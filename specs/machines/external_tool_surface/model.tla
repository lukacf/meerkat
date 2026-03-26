---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ExternalToolSurfaceMachine.

CONSTANTS NatValues, SurfaceDeltaOperationValues, SurfaceIdValues, TurnNumberValues

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

VARIABLES phase, model_step_count, known_surfaces, visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch

vars == << phase, model_step_count, known_surfaces, visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>

IsVisible(surface_id) == (surface_id \in visible_surfaces)
LastDeltaPhase(surface_id) == (IF ~((surface_id \in DOMAIN last_delta_phase)) THEN "None" ELSE (IF surface_id \in DOMAIN last_delta_phase THEN last_delta_phase[surface_id] ELSE "None"))
LastDeltaOperation(surface_id) == (IF ~((surface_id \in DOMAIN last_delta_operation)) THEN "None" ELSE (IF surface_id \in DOMAIN last_delta_operation THEN last_delta_operation[surface_id] ELSE "None"))
InflightCallCount(surface_id) == (IF ~((surface_id \in DOMAIN inflight_calls)) THEN 0 ELSE (IF surface_id \in DOMAIN inflight_calls THEN inflight_calls[surface_id] ELSE 0))
PendingLineageSequence(surface_id) == (IF ~((surface_id \in DOMAIN pending_lineage_sequence)) THEN 0 ELSE (IF surface_id \in DOMAIN pending_lineage_sequence THEN pending_lineage_sequence[surface_id] ELSE 0))
PendingTaskSequence(surface_id) == (IF ~((surface_id \in DOMAIN pending_task_sequence)) THEN 0 ELSE (IF surface_id \in DOMAIN pending_task_sequence THEN pending_task_sequence[surface_id] ELSE 0))
StagedIntentSequence(surface_id) == (IF ~((surface_id \in DOMAIN staged_intent_sequence)) THEN 0 ELSE (IF surface_id \in DOMAIN staged_intent_sequence THEN staged_intent_sequence[surface_id] ELSE 0))
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
    /\ staged_intent_sequence = [x \in {} |-> None]
    /\ next_staged_intent_sequence = 1
    /\ pending_task_sequence = [x \in {} |-> None]
    /\ pending_lineage_sequence = [x \in {} |-> None]
    /\ next_pending_task_sequence = 1
    /\ inflight_calls = [x \in {} |-> None]
    /\ last_delta_operation = [x \in {} |-> None]
    /\ last_delta_phase = [x \in {} |-> None]
    /\ snapshot_epoch = 0
    /\ snapshot_aligned_epoch = 0

TerminalStutter ==
    /\ phase = "Shutdown"
    /\ UNCHANGED vars

StageAdd(surface_id) ==
    /\ phase = "Operating"
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ staged_op' = MapSet(staged_op, surface_id, "Add")
    /\ staged_intent_sequence' = MapSet(staged_intent_sequence, surface_id, next_staged_intent_sequence)
    /\ next_staged_intent_sequence' = (next_staged_intent_sequence + 1)
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


StageRemove(surface_id) ==
    /\ phase = "Operating"
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ staged_op' = MapSet(staged_op, surface_id, "Remove")
    /\ staged_intent_sequence' = MapSet(staged_intent_sequence, surface_id, next_staged_intent_sequence)
    /\ next_staged_intent_sequence' = (next_staged_intent_sequence + 1)
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


StageReload(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ staged_op' = MapSet(staged_op, surface_id, "Reload")
    /\ staged_intent_sequence' = MapSet(staged_intent_sequence, surface_id, next_staged_intent_sequence)
    /\ next_staged_intent_sequence' = (next_staged_intent_sequence + 1)
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


ApplyBoundaryAdd(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Add")
    /\ (PendingOp(surface_id) = "None")
    /\ ((SurfaceBase(surface_id) = "Absent") \/ (SurfaceBase(surface_id) = "Active") \/ (SurfaceBase(surface_id) = "Removed"))
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "Add")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ staged_intent_sequence' = MapSet(staged_intent_sequence, surface_id, 0)
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, next_pending_task_sequence)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, StagedIntentSequence(surface_id))
    /\ next_pending_task_sequence' = (next_pending_task_sequence + 1)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Add")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Pending")
    /\ UNCHANGED << visible_surfaces, base_state, next_staged_intent_sequence, inflight_calls, snapshot_epoch, snapshot_aligned_epoch >>


ApplyBoundaryReload(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Reload")
    /\ (PendingOp(surface_id) = "None")
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "Reload")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ staged_intent_sequence' = MapSet(staged_intent_sequence, surface_id, 0)
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, next_pending_task_sequence)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, StagedIntentSequence(surface_id))
    /\ next_pending_task_sequence' = (next_pending_task_sequence + 1)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Reload")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Pending")
    /\ UNCHANGED << visible_surfaces, base_state, next_staged_intent_sequence, inflight_calls, snapshot_epoch, snapshot_aligned_epoch >>


ApplyBoundaryRemoveDraining(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Remove")
    /\ (PendingOp(surface_id) = "None")
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \ {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Removing")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ staged_intent_sequence' = MapSet(staged_intent_sequence, surface_id, 0)
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Remove")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Draining")
    /\ snapshot_epoch' = (IF (snapshot_epoch = snapshot_aligned_epoch) THEN (snapshot_epoch + 1) ELSE snapshot_epoch)
    /\ UNCHANGED << next_staged_intent_sequence, next_pending_task_sequence, inflight_calls, snapshot_aligned_epoch >>


ApplyBoundaryRemoveNoop(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (StagedOp(surface_id) = "Remove")
    /\ (PendingOp(surface_id) = "None")
    /\ (SurfaceBase(surface_id) # "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ staged_op' = MapSet(staged_op, surface_id, "None")
    /\ staged_intent_sequence' = MapSet(staged_intent_sequence, surface_id, 0)
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ UNCHANGED << visible_surfaces, base_state, next_staged_intent_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


PendingSucceededAdd(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (operation = "Add")
    /\ (PendingOp(surface_id) = operation)
    /\ (PendingTaskSequence(surface_id) = arg_pending_task_sequence)
    /\ (PendingLineageSequence(surface_id) = arg_staged_intent_sequence)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \cup {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Active")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Add")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Applied")
    /\ snapshot_epoch' = (IF (snapshot_epoch = snapshot_aligned_epoch) THEN (snapshot_epoch + 1) ELSE snapshot_epoch)
    /\ UNCHANGED << staged_op, staged_intent_sequence, next_staged_intent_sequence, next_pending_task_sequence, inflight_calls, snapshot_aligned_epoch >>


PendingSucceededReload(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (operation = "Reload")
    /\ (PendingOp(surface_id) = operation)
    /\ (PendingTaskSequence(surface_id) = arg_pending_task_sequence)
    /\ (PendingLineageSequence(surface_id) = arg_staged_intent_sequence)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \cup {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Active")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Reload")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Applied")
    /\ snapshot_epoch' = (IF (snapshot_epoch = snapshot_aligned_epoch) THEN (snapshot_epoch + 1) ELSE snapshot_epoch)
    /\ UNCHANGED << staged_op, staged_intent_sequence, next_staged_intent_sequence, next_pending_task_sequence, inflight_calls, snapshot_aligned_epoch >>


PendingFailedAdd(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (operation = "Add")
    /\ (PendingOp(surface_id) = operation)
    /\ (PendingTaskSequence(surface_id) = arg_pending_task_sequence)
    /\ (PendingLineageSequence(surface_id) = arg_staged_intent_sequence)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Add")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Failed")
    /\ UNCHANGED << visible_surfaces, base_state, staged_op, staged_intent_sequence, next_staged_intent_sequence, next_pending_task_sequence, inflight_calls, snapshot_epoch, snapshot_aligned_epoch >>


PendingFailedReload(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (operation = "Reload")
    /\ (PendingOp(surface_id) = operation)
    /\ (PendingTaskSequence(surface_id) = arg_pending_task_sequence)
    /\ (PendingLineageSequence(surface_id) = arg_staged_intent_sequence)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Reload")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Failed")
    /\ UNCHANGED << visible_surfaces, base_state, staged_op, staged_intent_sequence, next_staged_intent_sequence, next_pending_task_sequence, inflight_calls, snapshot_epoch, snapshot_aligned_epoch >>


CallStartedActive(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Active")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, (InflightCallCount(surface_id) + 1))
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


CallStartedRejectWhileRemoving(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Removing")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


CallStartedRejectWhileUnavailable(surface_id) ==
    /\ phase = "Operating"
    /\ ((SurfaceBase(surface_id) # "Active") /\ (SurfaceBase(surface_id) # "Removing"))
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


CallFinishedActive(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Active")
    /\ (InflightCallCount(surface_id) > 0)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, (InflightCallCount(surface_id) - 1))
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


CallFinishedRemoving(surface_id) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Removing")
    /\ (InflightCallCount(surface_id) > 0)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, (InflightCallCount(surface_id) - 1))
    /\ UNCHANGED << visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, last_delta_operation, last_delta_phase, snapshot_epoch, snapshot_aligned_epoch >>


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
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Remove")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Applied")
    /\ snapshot_epoch' = (IF (snapshot_epoch = snapshot_aligned_epoch) THEN (snapshot_epoch + 1) ELSE snapshot_epoch)
    /\ UNCHANGED << staged_op, staged_intent_sequence, next_staged_intent_sequence, next_pending_task_sequence, inflight_calls, snapshot_aligned_epoch >>


FinalizeRemovalForced(surface_id, applied_at_turn) ==
    /\ phase = "Operating"
    /\ (SurfaceBase(surface_id) = "Removing")
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = (known_surfaces \cup {surface_id})
    /\ visible_surfaces' = (visible_surfaces \ {surface_id})
    /\ base_state' = MapSet(base_state, surface_id, "Removed")
    /\ pending_op' = MapSet(pending_op, surface_id, "None")
    /\ pending_task_sequence' = MapSet(pending_task_sequence, surface_id, 0)
    /\ pending_lineage_sequence' = MapSet(pending_lineage_sequence, surface_id, 0)
    /\ inflight_calls' = MapSet(inflight_calls, surface_id, 0)
    /\ last_delta_operation' = MapSet(last_delta_operation, surface_id, "Remove")
    /\ last_delta_phase' = MapSet(last_delta_phase, surface_id, "Forced")
    /\ snapshot_epoch' = (IF (snapshot_epoch = snapshot_aligned_epoch) THEN (snapshot_epoch + 1) ELSE snapshot_epoch)
    /\ UNCHANGED << staged_op, staged_intent_sequence, next_staged_intent_sequence, next_pending_task_sequence, snapshot_aligned_epoch >>


SnapshotAligned(arg_snapshot_epoch) ==
    /\ phase = "Operating"
    /\ (arg_snapshot_epoch = snapshot_epoch)
    /\ (snapshot_epoch > snapshot_aligned_epoch)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ snapshot_aligned_epoch' = arg_snapshot_epoch
    /\ UNCHANGED << known_surfaces, visible_surfaces, base_state, pending_op, staged_op, staged_intent_sequence, next_staged_intent_sequence, pending_task_sequence, pending_lineage_sequence, next_pending_task_sequence, inflight_calls, last_delta_operation, last_delta_phase, snapshot_epoch >>


Shutdown ==
    /\ phase = "Operating" \/ phase = "Shutdown"
    /\ phase' = "Shutdown"
    /\ model_step_count' = model_step_count + 1
    /\ known_surfaces' = {}
    /\ visible_surfaces' = {}
    /\ base_state' = [x \in {} |-> None]
    /\ pending_op' = [x \in {} |-> None]
    /\ staged_op' = [x \in {} |-> None]
    /\ staged_intent_sequence' = [x \in {} |-> None]
    /\ next_staged_intent_sequence' = 1
    /\ pending_task_sequence' = [x \in {} |-> None]
    /\ pending_lineage_sequence' = [x \in {} |-> None]
    /\ next_pending_task_sequence' = 1
    /\ inflight_calls' = [x \in {} |-> None]
    /\ last_delta_operation' = [x \in {} |-> None]
    /\ last_delta_phase' = [x \in {} |-> None]
    /\ snapshot_epoch' = 0
    /\ snapshot_aligned_epoch' = 0


Next ==
    \/ \E surface_id \in SurfaceIdValues : StageAdd(surface_id)
    \/ \E surface_id \in SurfaceIdValues : StageRemove(surface_id)
    \/ \E surface_id \in SurfaceIdValues : StageReload(surface_id)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryAdd(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryReload(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryRemoveDraining(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : ApplyBoundaryRemoveNoop(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E operation \in SurfaceDeltaOperationValues : \E arg_pending_task_sequence \in 0..2 : \E arg_staged_intent_sequence \in 0..2 : \E applied_at_turn \in TurnNumberValues : PendingSucceededAdd(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E operation \in SurfaceDeltaOperationValues : \E arg_pending_task_sequence \in 0..2 : \E arg_staged_intent_sequence \in 0..2 : \E applied_at_turn \in TurnNumberValues : PendingSucceededReload(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E operation \in SurfaceDeltaOperationValues : \E arg_pending_task_sequence \in 0..2 : \E arg_staged_intent_sequence \in 0..2 : \E applied_at_turn \in TurnNumberValues : PendingFailedAdd(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E operation \in SurfaceDeltaOperationValues : \E arg_pending_task_sequence \in 0..2 : \E arg_staged_intent_sequence \in 0..2 : \E applied_at_turn \in TurnNumberValues : PendingFailedReload(surface_id, operation, arg_pending_task_sequence, arg_staged_intent_sequence, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : CallStartedActive(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallStartedRejectWhileRemoving(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallStartedRejectWhileUnavailable(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallFinishedActive(surface_id)
    \/ \E surface_id \in SurfaceIdValues : CallFinishedRemoving(surface_id)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : FinalizeRemovalClean(surface_id, applied_at_turn)
    \/ \E surface_id \in SurfaceIdValues : \E applied_at_turn \in TurnNumberValues : FinalizeRemovalForced(surface_id, applied_at_turn)
    \/ \E arg_snapshot_epoch \in 0..2 : SnapshotAligned(arg_snapshot_epoch)
    \/ Shutdown
    \/ TerminalStutter

visible_surfaces_subset_of_known_surfaces == (\A surface_id \in visible_surfaces : (surface_id \in known_surfaces))
base_state_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN base_state : (surface_id \in known_surfaces))
pending_op_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN pending_op : (surface_id \in known_surfaces))
staged_op_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN staged_op : (surface_id \in known_surfaces))
staged_intent_sequence_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN staged_intent_sequence : (surface_id \in known_surfaces))
pending_task_sequence_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN pending_task_sequence : (surface_id \in known_surfaces))
pending_lineage_sequence_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN pending_lineage_sequence : (surface_id \in known_surfaces))
inflight_calls_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN inflight_calls : (surface_id \in known_surfaces))
last_delta_operation_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN last_delta_operation : (surface_id \in known_surfaces))
last_delta_phase_keys_subset_of_known_surfaces == (\A surface_id \in DOMAIN last_delta_phase : (surface_id \in known_surfaces))
removing_or_removed_surfaces_are_not_visible == (\A surface_id \in known_surfaces : (((SurfaceBase(surface_id) = "Removing") /\ ~(IsVisible(surface_id))) \/ ((SurfaceBase(surface_id) = "Removed") /\ ~(IsVisible(surface_id))) \/ ((SurfaceBase(surface_id) # "Removing") /\ (SurfaceBase(surface_id) # "Removed"))))
visible_membership_matches_active_base_state == (\A surface_id \in known_surfaces : (IsVisible(surface_id) = (SurfaceBase(surface_id) = "Active")))
removing_surfaces_have_no_pending_add_or_reload == (\A surface_id \in known_surfaces : ((SurfaceBase(surface_id) # "Removing") \/ (PendingOp(surface_id) = "None")))
removed_surfaces_only_allow_pending_none_or_add == (\A surface_id \in known_surfaces : ((SurfaceBase(surface_id) # "Removed") \/ ((PendingOp(surface_id) = "None") \/ (PendingOp(surface_id) = "Add"))))
inflight_calls_only_exist_for_active_or_removing_surfaces == (\A surface_id \in known_surfaces : ((InflightCallCount(surface_id) = 0) \/ ((SurfaceBase(surface_id) = "Active") \/ (SurfaceBase(surface_id) = "Removing"))))
reload_pending_requires_active_base_state == (\A surface_id \in known_surfaces : ((PendingOp(surface_id) # "Reload") \/ (SurfaceBase(surface_id) = "Active")))
removed_surfaces_have_zero_inflight_calls == (\A surface_id \in known_surfaces : ((SurfaceBase(surface_id) # "Removed") \/ (InflightCallCount(surface_id) = 0)))
forced_delta_phase_is_always_a_remove_delta == (\A surface_id \in known_surfaces : ((LastDeltaPhase(surface_id) # "Forced") \/ (LastDeltaOperation(surface_id) = "Remove")))
staged_sequence_matches_staged_presence == (\A surface_id \in known_surfaces : (((StagedOp(surface_id) = "None") /\ (StagedIntentSequence(surface_id) = 0)) \/ ((StagedOp(surface_id) # "None") /\ (StagedIntentSequence(surface_id) > 0))))
pending_lineage_matches_pending_presence == (\A surface_id \in known_surfaces : (((PendingOp(surface_id) = "None") /\ (PendingTaskSequence(surface_id) = 0) /\ (PendingLineageSequence(surface_id) = 0)) \/ ((PendingOp(surface_id) # "None") /\ (PendingTaskSequence(surface_id) > 0) /\ (PendingLineageSequence(surface_id) > 0))))
snapshot_alignment_epoch_not_ahead == (snapshot_aligned_epoch <= snapshot_epoch)

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(known_surfaces) <= 1 /\ Cardinality(visible_surfaces) <= 1 /\ Cardinality(DOMAIN base_state) <= 1 /\ Cardinality(DOMAIN pending_op) <= 1 /\ Cardinality(DOMAIN staged_op) <= 1 /\ Cardinality(DOMAIN staged_intent_sequence) <= 1 /\ Cardinality(DOMAIN pending_task_sequence) <= 1 /\ Cardinality(DOMAIN pending_lineage_sequence) <= 1 /\ Cardinality(DOMAIN inflight_calls) <= 1 /\ Cardinality(DOMAIN last_delta_operation) <= 1 /\ Cardinality(DOMAIN last_delta_phase) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(known_surfaces) <= 2 /\ Cardinality(visible_surfaces) <= 2 /\ Cardinality(DOMAIN base_state) <= 2 /\ Cardinality(DOMAIN pending_op) <= 2 /\ Cardinality(DOMAIN staged_op) <= 2 /\ Cardinality(DOMAIN staged_intent_sequence) <= 2 /\ Cardinality(DOMAIN pending_task_sequence) <= 2 /\ Cardinality(DOMAIN pending_lineage_sequence) <= 2 /\ Cardinality(DOMAIN inflight_calls) <= 2 /\ Cardinality(DOMAIN last_delta_operation) <= 2 /\ Cardinality(DOMAIN last_delta_phase) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []visible_surfaces_subset_of_known_surfaces
THEOREM Spec => []base_state_keys_subset_of_known_surfaces
THEOREM Spec => []pending_op_keys_subset_of_known_surfaces
THEOREM Spec => []staged_op_keys_subset_of_known_surfaces
THEOREM Spec => []staged_intent_sequence_keys_subset_of_known_surfaces
THEOREM Spec => []pending_task_sequence_keys_subset_of_known_surfaces
THEOREM Spec => []pending_lineage_sequence_keys_subset_of_known_surfaces
THEOREM Spec => []inflight_calls_keys_subset_of_known_surfaces
THEOREM Spec => []last_delta_operation_keys_subset_of_known_surfaces
THEOREM Spec => []last_delta_phase_keys_subset_of_known_surfaces
THEOREM Spec => []removing_or_removed_surfaces_are_not_visible
THEOREM Spec => []visible_membership_matches_active_base_state
THEOREM Spec => []removing_surfaces_have_no_pending_add_or_reload
THEOREM Spec => []removed_surfaces_only_allow_pending_none_or_add
THEOREM Spec => []inflight_calls_only_exist_for_active_or_removing_surfaces
THEOREM Spec => []reload_pending_requires_active_base_state
THEOREM Spec => []removed_surfaces_have_zero_inflight_calls
THEOREM Spec => []forced_delta_phase_is_always_a_remove_delta
THEOREM Spec => []staged_sequence_matches_staged_presence
THEOREM Spec => []pending_lineage_matches_pending_presence
THEOREM Spec => []snapshot_alignment_epoch_not_ahead

=============================================================================
