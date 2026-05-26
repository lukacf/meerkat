---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkAttentionLifecycleMachine.

CONSTANTS NatValues, WorkAttentionBindingKeyValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
OptionWorkAttentionBindingKeyValues == {None} \cup {Some(x) : x \in WorkAttentionBindingKeyValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms

vars == << phase, model_step_count, revision, paused_until_utc_ms, superseded_by_binding_key, terminal_at_utc_ms >>

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ revision = 1
    /\ paused_until_utc_ms = None
    /\ superseded_by_binding_key = None
    /\ terminal_at_utc_ms = None

TerminalStutter ==
    /\ phase = "Superseded" \/ phase = "Stopped"
    /\ UNCHANGED vars

PauseActive(expected_revision, until_utc_ms) ==
    /\ phase = "Active"
    /\ (revision = expected_revision)
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = until_utc_ms
    /\ UNCHANGED << superseded_by_binding_key, terminal_at_utc_ms >>


PausePaused(expected_revision, until_utc_ms) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Paused"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = until_utc_ms
    /\ UNCHANGED << superseded_by_binding_key, terminal_at_utc_ms >>


ResumePaused(expected_revision) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ UNCHANGED << superseded_by_binding_key, terminal_at_utc_ms >>


SupersedeActive(expected_revision, arg_superseded_by_binding_key, at_utc_ms) ==
    /\ phase = "Active"
    /\ (revision = expected_revision)
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ superseded_by_binding_key' = Some(arg_superseded_by_binding_key)
    /\ terminal_at_utc_ms' = Some(at_utc_ms)


SupersedePaused(expected_revision, arg_superseded_by_binding_key, at_utc_ms) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ superseded_by_binding_key' = Some(arg_superseded_by_binding_key)
    /\ terminal_at_utc_ms' = Some(at_utc_ms)


StopActive(expected_revision, at_utc_ms) ==
    /\ phase = "Active"
    /\ (revision = expected_revision)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << superseded_by_binding_key >>


StopPaused(expected_revision, at_utc_ms) ==
    /\ phase = "Paused"
    /\ (revision = expected_revision)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ paused_until_utc_ms' = None
    /\ terminal_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << superseded_by_binding_key >>


Next ==
    \/ \E expected_revision \in 0..2 : \E until_utc_ms \in OptionU64Values : PauseActive(expected_revision, until_utc_ms)
    \/ \E expected_revision \in 0..2 : \E until_utc_ms \in OptionU64Values : PausePaused(expected_revision, until_utc_ms)
    \/ \E expected_revision \in 0..2 : ResumePaused(expected_revision)
    \/ \E expected_revision \in 0..2 : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E at_utc_ms \in 0..2 : SupersedeActive(expected_revision, arg_superseded_by_binding_key, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E arg_superseded_by_binding_key \in WorkAttentionBindingKeyValues : \E at_utc_ms \in 0..2 : SupersedePaused(expected_revision, arg_superseded_by_binding_key, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : StopActive(expected_revision, at_utc_ms)
    \/ \E expected_revision \in 0..2 : \E at_utc_ms \in 0..2 : StopPaused(expected_revision, at_utc_ms)
    \/ TerminalStutter

live_has_no_terminal_time == (((phase # "Active") /\ (phase # "Paused")) \/ (terminal_at_utc_ms = None))
paused_has_pause_state == ((phase = "Paused") \/ (paused_until_utc_ms = None))
superseded_records_successor == ((phase # "Superseded") \/ (superseded_by_binding_key # None))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []live_has_no_terminal_time
THEOREM Spec => []paused_has_pause_state
THEOREM Spec => []superseded_records_successor

=============================================================================
