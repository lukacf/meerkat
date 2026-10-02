---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for WorkItemAdmissionMachine.

CONSTANTS BooleanValues, WorkAdmissionDigestRefValues, WorkAdmissionKeyRefValues, WorkAdmissionReplayKindValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionWorkAdmissionDigestRefValues == {None} \cup {Some(x) : x \in WorkAdmissionDigestRefValues}
OptionWorkAdmissionKeyRefValues == {None} \cup {Some(x) : x \in WorkAdmissionKeyRefValues}

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

VARIABLES phase, model_step_count, admission_key, request_digest

vars == << phase, model_step_count, admission_key, request_digest >>

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ admission_key = None
    /\ request_digest = None

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_fe66b5913fc82c17 == UNCHANGED << admission_key, request_digest >>

BindKeyed(arg_admission_key, arg_request_digest) ==
    /\ phase = "Absent"
    /\ ((arg_admission_key # None) /\ (arg_request_digest # None))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ admission_key' = arg_admission_key
    /\ request_digest' = arg_request_digest


BindUnkeyed(arg_admission_key, arg_request_digest) ==
    /\ phase = "Absent"
    /\ ((arg_admission_key = None) /\ (arg_request_digest = None))
    /\ phase' = "Unkeyed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


ClassifyAdmissionReplayExactUnkeyed(requested_admission_key, requested_request_digest) ==
    /\ phase = "Unkeyed"
    /\ ((admission_key = Some(requested_admission_key)) /\ (request_digest = Some(requested_request_digest)))
    /\ phase' = "Unkeyed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


ClassifyAdmissionReplayExactAdmitted(requested_admission_key, requested_request_digest) ==
    /\ phase = "Admitted"
    /\ ((admission_key = Some(requested_admission_key)) /\ (request_digest = Some(requested_request_digest)))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


ClassifyAdmissionReplayConflictUnkeyed(requested_admission_key, requested_request_digest) ==
    /\ phase = "Unkeyed"
    /\ ((admission_key = Some(requested_admission_key)) /\ (request_digest # Some(requested_request_digest)))
    /\ phase' = "Unkeyed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


ClassifyAdmissionReplayConflictAdmitted(requested_admission_key, requested_request_digest) ==
    /\ phase = "Admitted"
    /\ ((admission_key = Some(requested_admission_key)) /\ (request_digest # Some(requested_request_digest)))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


ClassifyAdmissionReplayKeyMismatchAbsent(requested_admission_key, requested_request_digest) ==
    /\ phase = "Absent"
    /\ (admission_key # Some(requested_admission_key))
    /\ phase' = "Absent"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


ClassifyAdmissionReplayKeyMismatchUnkeyed(requested_admission_key, requested_request_digest) ==
    /\ phase = "Unkeyed"
    /\ (admission_key # Some(requested_admission_key))
    /\ phase' = "Unkeyed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


ClassifyAdmissionReplayKeyMismatchAdmitted(requested_admission_key, requested_request_digest) ==
    /\ phase = "Admitted"
    /\ (admission_key # Some(requested_admission_key))
    /\ phase' = "Admitted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fe66b5913fc82c17


Next ==
    \/ \E arg_admission_key \in OptionWorkAdmissionKeyRefValues : \E arg_request_digest \in OptionWorkAdmissionDigestRefValues : BindKeyed(arg_admission_key, arg_request_digest)
    \/ \E arg_admission_key \in OptionWorkAdmissionKeyRefValues : \E arg_request_digest \in OptionWorkAdmissionDigestRefValues : BindUnkeyed(arg_admission_key, arg_request_digest)
    \/ \E requested_admission_key \in WorkAdmissionKeyRefValues : \E requested_request_digest \in WorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactUnkeyed(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in WorkAdmissionKeyRefValues : \E requested_request_digest \in WorkAdmissionDigestRefValues : ClassifyAdmissionReplayExactAdmitted(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in WorkAdmissionKeyRefValues : \E requested_request_digest \in WorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictUnkeyed(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in WorkAdmissionKeyRefValues : \E requested_request_digest \in WorkAdmissionDigestRefValues : ClassifyAdmissionReplayConflictAdmitted(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in WorkAdmissionKeyRefValues : \E requested_request_digest \in WorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchAbsent(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in WorkAdmissionKeyRefValues : \E requested_request_digest \in WorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchUnkeyed(requested_admission_key, requested_request_digest)
    \/ \E requested_admission_key \in WorkAdmissionKeyRefValues : \E requested_request_digest \in WorkAdmissionDigestRefValues : ClassifyAdmissionReplayKeyMismatchAdmitted(requested_admission_key, requested_request_digest)

admitted_has_identity == (IF (phase # "Admitted") THEN TRUE ELSE ((admission_key # None) /\ (request_digest # None)))
non_admitted_has_no_identity == (IF (phase = "Admitted") THEN TRUE ELSE ((admission_key = None) /\ (request_digest = None)))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []admitted_has_identity
THEOREM Spec => []non_admitted_has_no_identity

=============================================================================
