---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for PendingContinuationAdmissionMachine.

CONSTANTS NatValues, ObservedSessionTailKindValues, PendingContinuationDispositionValues, PendingContinuationPublicTerminalValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionPendingContinuationPublicTerminalValues == {None} \cup {Some(x) : x \in PendingContinuationPublicTerminalValues}

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

VARIABLES phase, model_step_count, last_public_terminal

vars == << phase, model_step_count, last_public_terminal >>

tail_has_pending_boundary(session_tail) == ((session_tail = "User") \/ (session_tail = "ToolResults"))
has_effective_pending_boundary(session_tail, staged_tool_result_count) == (tail_has_pending_boundary(session_tail) \/ (staged_tool_result_count > 0))

Init ==
    /\ phase = "Ready"
    /\ model_step_count = 0
    /\ last_public_terminal = None

ResolveWithBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ has_effective_pending_boundary(session_tail, staged_tool_result_count)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = None


ResolveWithoutBoundary(session_tail, staged_tool_result_count) ==
    /\ phase = "Ready"
    /\ (has_effective_pending_boundary(session_tail, staged_tool_result_count) = FALSE)
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ last_public_terminal' = Some("NoPendingBoundary")


ResolveLastNoPendingTerminal ==
    /\ phase = "Ready"
    /\ (last_public_terminal = Some("NoPendingBoundary"))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << last_public_terminal >>


Next ==
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolveWithBoundary(session_tail, staged_tool_result_count)
    \/ \E session_tail \in ObservedSessionTailKindValues : \E staged_tool_result_count \in 0..2 : ResolveWithoutBoundary(session_tail, staged_tool_result_count)
    \/ ResolveLastNoPendingTerminal


CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars


=============================================================================
