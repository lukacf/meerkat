---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for SessionToolVisibilityMachine.

CONSTANTS SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringToolVisibilityWitnessValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ToolVisibilityWitnessValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_revision, staged_revision

vars == << phase, model_step_count, inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_revision, staged_revision >>

FilterWitnessKeys == DOMAIN filter_witnesses
RequestedWitnessKeys == DOMAIN requested_witnesses
HasPendingPromotion == (staged_revision > active_revision)

Init ==
    /\ phase = "Operating"
    /\ model_step_count = 0
    /\ inherited_base_filter = "All"
    /\ active_filter = "All"
    /\ staged_filter = "All"
    /\ active_requested_deferred_names = {}
    /\ staged_requested_deferred_names = {}
    /\ requested_witnesses = [x \in {} |-> None]
    /\ filter_witnesses = [x \in {} |-> None]
    /\ active_revision = 0
    /\ staged_revision = 0

RECURSIVE StagePersistentFilter_ForEach0_filter_witnesses(_, _, _)
StagePersistentFilter_ForEach0_filter_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN StagePersistentFilter_ForEach0_filter_witnesses(next_acc, remaining \ {item}, outer_witnesses)

RECURSIVE RequestDeferredTools_ForEach1_staged_requested_deferred_names(_, _)
RequestDeferredTools_ForEach1_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredTools_ForEach1_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE RequestDeferredTools_ForEach2_requested_witnesses(_, _, _)
RequestDeferredTools_ForEach2_requested_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN RequestDeferredTools_ForEach2_requested_witnesses(next_acc, remaining \ {item}, outer_witnesses)

StagePersistentFilter(filter, witnesses) ==
    /\ phase = "Operating"
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ staged_filter' = filter
    /\ filter_witnesses' = StagePersistentFilter_ForEach0_filter_witnesses(filter_witnesses, DOMAIN witnesses, witnesses)
    /\ staged_revision' = (staged_revision) + 1
    /\ UNCHANGED << inherited_base_filter, active_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, active_revision >>


RequestDeferredTools(names, witnesses) ==
    /\ phase = "Operating"
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredTools_ForEach1_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ requested_witnesses' = RequestDeferredTools_ForEach2_requested_witnesses(requested_witnesses, DOMAIN witnesses, witnesses)
    /\ staged_revision' = (staged_revision) + 1
    /\ UNCHANGED << inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, filter_witnesses, active_revision >>


ApplyBoundaryPromote ==
    /\ phase = "Operating"
    /\ HasPendingPromotion
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ active_filter' = staged_filter
    /\ active_requested_deferred_names' = staged_requested_deferred_names
    /\ active_revision' = staged_revision
    /\ UNCHANGED << inherited_base_filter, staged_filter, staged_requested_deferred_names, requested_witnesses, filter_witnesses, staged_revision >>


ApplyBoundaryNoop ==
    /\ phase = "Operating"
    /\ ~(HasPendingPromotion)
    /\ phase' = "Operating"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << inherited_base_filter, active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, requested_witnesses, filter_witnesses, active_revision, staged_revision >>


Next ==
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilter(filter, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredTools(names, witnesses)
    \/ ApplyBoundaryPromote
    \/ ApplyBoundaryNoop

active_revision_not_ahead_of_staged == (active_revision <= staged_revision)
active_requested_names_subset_of_staged == (\A name \in active_requested_deferred_names : (name \in staged_requested_deferred_names))
equal_revision_means_equal_active_and_staged_state == ((active_revision # staged_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(active_requested_deferred_names) <= 1 /\ Cardinality(staged_requested_deferred_names) <= 1 /\ Cardinality(DOMAIN requested_witnesses) <= 1 /\ Cardinality(DOMAIN filter_witnesses) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(active_requested_deferred_names) <= 2 /\ Cardinality(staged_requested_deferred_names) <= 2 /\ Cardinality(DOMAIN requested_witnesses) <= 2 /\ Cardinality(DOMAIN filter_witnesses) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []active_revision_not_ahead_of_staged
THEOREM Spec => []active_requested_names_subset_of_staged
THEOREM Spec => []equal_revision_means_equal_active_and_staged_state

=============================================================================
