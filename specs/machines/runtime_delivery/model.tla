---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for RuntimeDeliveryMachine.

\* RustU64Max is the TLA boundary for Expr::U64Max; production generated Rust renders u64::MAX.
CONSTANTS NatValues, SetOfStringValues, SetOfU64Values, StringValues, RustU64Max

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in NatValues }

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

VARIABLES phase, model_step_count, delivery_ids, delivery_sequences, delivery_source_sequences, committed_sequences, next_sequence, applied_cursor

vars == << phase, model_step_count, delivery_ids, delivery_sequences, delivery_source_sequences, committed_sequences, next_sequence, applied_cursor >>

Init ==
    /\ phase = "Active"
    /\ model_step_count = 0
    /\ delivery_ids = {}
    /\ delivery_sequences = [x \in {} |-> None]
    /\ delivery_source_sequences = [x \in {} |-> None]
    /\ committed_sequences = {}
    /\ next_sequence = 0
    /\ applied_cursor = 0

CommitNewDelivery(delivery_id, source_sequence) ==
    /\ phase = "Active"
    /\ (((delivery_id \in delivery_ids) = FALSE) /\ (source_sequence > 0) /\ (next_sequence < RustU64Max))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_ids' = (delivery_ids \cup {delivery_id})
    /\ delivery_sequences' = MapSet(delivery_sequences, delivery_id, (next_sequence) + 1)
    /\ delivery_source_sequences' = MapSet(delivery_source_sequences, delivery_id, source_sequence)
    /\ committed_sequences' = (committed_sequences \cup {(next_sequence) + 1})
    /\ next_sequence' = (next_sequence) + 1
    /\ UNCHANGED << applied_cursor >>


ReuseCommittedDelivery(delivery_id, source_sequence) ==
    /\ phase = "Active"
    /\ ((delivery_id \in delivery_ids) /\ ((IF "value" \in DOMAIN (IF (delivery_id \in DOMAIN delivery_source_sequences) THEN Some((IF delivery_id \in DOMAIN delivery_source_sequences THEN delivery_source_sequences[delivery_id] ELSE 0)) ELSE None) THEN (IF (delivery_id \in DOMAIN delivery_source_sequences) THEN Some((IF delivery_id \in DOMAIN delivery_source_sequences THEN delivery_source_sequences[delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = source_sequence))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << delivery_ids, delivery_sequences, delivery_source_sequences, committed_sequences, next_sequence, applied_cursor >>


ApplyNextDelivery(delivery_id, delivery_sequence) ==
    /\ phase = "Active"
    /\ ((delivery_id \in delivery_ids) /\ ((IF "value" \in DOMAIN (IF (delivery_id \in DOMAIN delivery_sequences) THEN Some((IF delivery_id \in DOMAIN delivery_sequences THEN delivery_sequences[delivery_id] ELSE 0)) ELSE None) THEN (IF (delivery_id \in DOMAIN delivery_sequences) THEN Some((IF delivery_id \in DOMAIN delivery_sequences THEN delivery_sequences[delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = delivery_sequence) /\ (delivery_sequence > applied_cursor) /\ ((delivery_sequence - 1) = applied_cursor))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ applied_cursor' = delivery_sequence
    /\ UNCHANGED << delivery_ids, delivery_sequences, delivery_source_sequences, committed_sequences, next_sequence >>


ObserveAlreadyAppliedDelivery(delivery_id, delivery_sequence) ==
    /\ phase = "Active"
    /\ ((delivery_id \in delivery_ids) /\ ((IF "value" \in DOMAIN (IF (delivery_id \in DOMAIN delivery_sequences) THEN Some((IF delivery_id \in DOMAIN delivery_sequences THEN delivery_sequences[delivery_id] ELSE 0)) ELSE None) THEN (IF (delivery_id \in DOMAIN delivery_sequences) THEN Some((IF delivery_id \in DOMAIN delivery_sequences THEN delivery_sequences[delivery_id] ELSE 0)) ELSE None)["value"] ELSE None) = delivery_sequence) /\ (delivery_sequence <= applied_cursor))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << delivery_ids, delivery_sequences, delivery_source_sequences, committed_sequences, next_sequence, applied_cursor >>


Next ==
    \/ \E delivery_id \in StringValues : \E source_sequence \in 0..2 : CommitNewDelivery(delivery_id, source_sequence)
    \/ \E delivery_id \in StringValues : \E source_sequence \in 0..2 : ReuseCommittedDelivery(delivery_id, source_sequence)
    \/ \E delivery_id \in StringValues : \E delivery_sequence \in 0..2 : ApplyNextDelivery(delivery_id, delivery_sequence)
    \/ \E delivery_id \in StringValues : \E delivery_sequence \in 0..2 : ObserveAlreadyAppliedDelivery(delivery_id, delivery_sequence)

applied_cursor_does_not_pass_committed_sequence == (applied_cursor <= next_sequence)
empty_delivery_set_has_zero_sequence == (IF (Cardinality(delivery_ids) # 0) THEN TRUE ELSE (next_sequence = 0))
delivery_identity_and_sequence_cardinality_match == (Cardinality(delivery_ids) = Cardinality(committed_sequences))
committed_sequence_cardinality_tracks_high_water == (Cardinality(committed_sequences) = next_sequence)

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(delivery_ids) <= 1 /\ Cardinality(DOMAIN delivery_sequences) <= 1 /\ Cardinality(DOMAIN delivery_source_sequences) <= 1 /\ Cardinality(committed_sequences) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(delivery_ids) <= 2 /\ Cardinality(DOMAIN delivery_sequences) <= 2 /\ Cardinality(DOMAIN delivery_source_sequences) <= 2 /\ Cardinality(committed_sequences) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []applied_cursor_does_not_pass_committed_sequence
THEOREM Spec => []empty_delivery_set_has_zero_sequence
THEOREM Spec => []delivery_identity_and_sequence_cardinality_match
THEOREM Spec => []committed_sequence_cardinality_tracks_high_water

=============================================================================
