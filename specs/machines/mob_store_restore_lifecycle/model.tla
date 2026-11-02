---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobStoreRestoreLifecycleMachine.

CONSTANTS MobQuarantineClassValues, MobStoreRestoreOutcomeValues, MobStoreRestoreRefusalValues, MobStoreRetryTriggerValues, NatValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

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

VARIABLES phase, model_step_count, class, attempts

vars == << phase, model_step_count, class, attempts >>

Init ==
    /\ phase = "Unclassified"
    /\ model_step_count = 0
    /\ class = "StoreUnavailable"
    /\ attempts = 0

TerminalStutter ==
    /\ phase = "Released"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_509a68a7be56c2b8 == UNCHANGED << class, attempts >>
UnchangedFrame_c7a4ebbf1e9e71e4 == UNCHANGED << attempts >>
UnchangedFrame_d86b8a73fa57dab6 == UNCHANGED << class >>

QuarantineUnclassified(arg_class) ==
    /\ phase = "Unclassified"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ class' = arg_class
    /\ UnchangedFrame_c7a4ebbf1e9e71e4


QuarantineQuarantined(arg_class) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


QuarantineRetrying(arg_class) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


QuarantineReleased(arg_class) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


RequestRetryUnclassified(trigger) ==
    /\ phase = "Unclassified"
    /\ phase' = "Unclassified"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


RequestRetryQuarantined(trigger) ==
    /\ phase = "Quarantined"
    /\ (IF (trigger = "Explicit") THEN TRUE ELSE (class = "StoreUnavailable"))
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


RequestRetryQuarantinedNotRetryable(trigger) ==
    /\ phase = "Quarantined"
    /\ (trigger = "OnDemand")
    /\ (class # "StoreUnavailable")
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


RequestRetryRetrying(trigger) ==
    /\ phase = "Retrying"
    /\ phase' = "Retrying"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


RequestRetryReleased(trigger) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


ResolveRetryFailed(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ (outcome = "Failed")
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ class' = arg_class
    /\ attempts' = (attempts) + 1


ResolveRetryRestored(outcome, arg_class) ==
    /\ phase = "Retrying"
    /\ (outcome # "Failed")
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ attempts' = (attempts) + 1
    /\ UnchangedFrame_d86b8a73fa57dab6


ResolveRetryUnclassified(outcome, arg_class) ==
    /\ phase = "Unclassified"
    /\ phase' = "Unclassified"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


ResolveRetryQuarantined(outcome, arg_class) ==
    /\ phase = "Quarantined"
    /\ phase' = "Quarantined"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


ResolveRetryReleased(outcome, arg_class) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_509a68a7be56c2b8


Next ==
    \/ (phase = "Unclassified") /\ \E arg_class \in MobQuarantineClassValues : QuarantineUnclassified(arg_class)
    \/ (phase = "Quarantined") /\ \E arg_class \in MobQuarantineClassValues : QuarantineQuarantined(arg_class)
    \/ (phase = "Retrying") /\ \E arg_class \in MobQuarantineClassValues : QuarantineRetrying(arg_class)
    \/ (phase = "Released") /\ \E arg_class \in MobQuarantineClassValues : QuarantineReleased(arg_class)
    \/ (phase = "Unclassified") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryUnclassified(trigger)
    \/ (phase = "Quarantined") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryQuarantined(trigger)
    \/ (phase = "Quarantined") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryQuarantinedNotRetryable(trigger)
    \/ (phase = "Retrying") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryRetrying(trigger)
    \/ (phase = "Released") /\ \E trigger \in MobStoreRetryTriggerValues : RequestRetryReleased(trigger)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryFailed(outcome, arg_class)
    \/ (phase = "Retrying") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryRestored(outcome, arg_class)
    \/ (phase = "Unclassified") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryUnclassified(outcome, arg_class)
    \/ (phase = "Quarantined") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryQuarantined(outcome, arg_class)
    \/ (phase = "Released") /\ \E outcome \in MobStoreRestoreOutcomeValues : \E arg_class \in MobQuarantineClassValues : ResolveRetryReleased(outcome, arg_class)
    \/ TerminalStutter

unclassified_record_has_no_attempts == (IF (phase # "Unclassified") THEN TRUE ELSE (attempts = 0))
released_record_was_attempted == (IF (phase # "Released") THEN TRUE ELSE (attempts > 0))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []unclassified_record_has_no_attempts
THEOREM Spec => []released_record_was_attempted

=============================================================================
