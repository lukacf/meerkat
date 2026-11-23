---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobHostedSetLifecycleMachine.

CONSTANTS BooleanValues, MobHostedSetRefusalValues

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

VARIABLES phase, model_step_count, census_owed, census_complete, host_inserted, all_mobs_known

vars == << phase, model_step_count, census_owed, census_complete, host_inserted, all_mobs_known >>

Init ==
    /\ phase = "Unrestored"
    /\ model_step_count = 0
    /\ census_owed = FALSE
    /\ census_complete = FALSE
    /\ host_inserted = FALSE
    /\ all_mobs_known = TRUE

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_067cfbcaa106fb1c == UNCHANGED << census_owed, census_complete, host_inserted, all_mobs_known >>
UnchangedFrame_0f4487d75d2b9e78 == UNCHANGED << census_owed, census_complete >>
UnchangedFrame_25cf8aca8dc2204c == UNCHANGED << census_owed, host_inserted >>
UnchangedFrame_3cf95763105f9195 == UNCHANGED << census_complete, host_inserted, all_mobs_known >>
UnchangedFrame_c5ddd21b135b8e44 == UNCHANGED << census_complete, host_inserted >>
UnchangedFrame_fe440a7fd11975c1 == UNCHANGED << census_owed, census_complete, all_mobs_known >>

ExpectCensusOwed(owed) ==
    /\ phase = "Unrestored"
    /\ (owed = TRUE)
    /\ (census_complete = FALSE)
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ census_owed' = TRUE
    /\ all_mobs_known' = FALSE
    /\ UnchangedFrame_c5ddd21b135b8e44


ExpectCensusOwedComplete(owed) ==
    /\ phase = "Unrestored"
    /\ (owed = TRUE)
    /\ (census_complete = TRUE)
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ census_owed' = TRUE
    /\ UnchangedFrame_3cf95763105f9195


ExpectCensusNotOwed(owed) ==
    /\ phase = "Unrestored"
    /\ (owed = FALSE)
    /\ (IF (census_complete = TRUE) THEN TRUE ELSE (host_inserted = FALSE))
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ census_owed' = FALSE
    /\ all_mobs_known' = TRUE
    /\ UnchangedFrame_c5ddd21b135b8e44


ExpectCensusNotOwedHostInserts(owed) ==
    /\ phase = "Unrestored"
    /\ (owed = FALSE)
    /\ ((census_complete = FALSE) /\ (host_inserted = TRUE))
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ census_owed' = FALSE
    /\ all_mobs_known' = FALSE
    /\ UnchangedFrame_c5ddd21b135b8e44


ExpectCensusAttempted(owed) ==
    /\ phase = "Attempted"
    /\ phase' = "Attempted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


ExpectCensusRestored(owed) ==
    /\ phase = "Restored"
    /\ phase' = "Restored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


BeginPassFirst ==
    /\ phase = "Unrestored"
    /\ (census_owed = TRUE)
    /\ phase' = "Attempted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


BeginPassNoCensusOwed ==
    /\ phase = "Unrestored"
    /\ (census_owed = FALSE)
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


BeginPassAgain ==
    /\ phase = "Attempted"
    /\ phase' = "Attempted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


BeginPassRestored ==
    /\ phase = "Restored"
    /\ phase' = "Restored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


FinishPassCompletesCensus ==
    /\ phase = "Attempted"
    /\ (census_complete = FALSE)
    /\ phase' = "Restored"
    /\ model_step_count' = model_step_count + 1
    /\ census_complete' = TRUE
    /\ all_mobs_known' = TRUE
    /\ UnchangedFrame_25cf8aca8dc2204c


FinishPassCensusComplete ==
    /\ phase = "Attempted"
    /\ (census_complete = TRUE)
    /\ phase' = "Restored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


FinishPassUnrestored ==
    /\ phase = "Unrestored"
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


FinishPassRestored ==
    /\ phase = "Restored"
    /\ phase' = "Restored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


DeclareRestoredUnrestored ==
    /\ phase = "Unrestored"
    /\ (census_complete = FALSE)
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ census_complete' = TRUE
    /\ all_mobs_known' = TRUE
    /\ UnchangedFrame_25cf8aca8dc2204c


DeclareRestoredUnrestoredComplete ==
    /\ phase = "Unrestored"
    /\ (census_complete = TRUE)
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


DeclareRestoredAttempted ==
    /\ phase = "Attempted"
    /\ (census_complete = FALSE)
    /\ phase' = "Attempted"
    /\ model_step_count' = model_step_count + 1
    /\ census_complete' = TRUE
    /\ all_mobs_known' = TRUE
    /\ UnchangedFrame_25cf8aca8dc2204c


DeclareRestoredAttemptedComplete ==
    /\ phase = "Attempted"
    /\ (census_complete = TRUE)
    /\ phase' = "Attempted"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


DeclareRestoredRestored ==
    /\ phase = "Restored"
    /\ phase' = "Restored"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_067cfbcaa106fb1c


RegisterHostHandleUnrestored ==
    /\ phase = "Unrestored"
    /\ (census_complete = FALSE)
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ host_inserted' = TRUE
    /\ all_mobs_known' = FALSE
    /\ UnchangedFrame_0f4487d75d2b9e78


RegisterHostHandleUnrestoredComplete ==
    /\ phase = "Unrestored"
    /\ (census_complete = TRUE)
    /\ phase' = "Unrestored"
    /\ model_step_count' = model_step_count + 1
    /\ host_inserted' = TRUE
    /\ UnchangedFrame_fe440a7fd11975c1


RegisterHostHandleAttempted ==
    /\ phase = "Attempted"
    /\ (census_complete = FALSE)
    /\ phase' = "Attempted"
    /\ model_step_count' = model_step_count + 1
    /\ host_inserted' = TRUE
    /\ all_mobs_known' = FALSE
    /\ UnchangedFrame_0f4487d75d2b9e78


RegisterHostHandleAttemptedComplete ==
    /\ phase = "Attempted"
    /\ (census_complete = TRUE)
    /\ phase' = "Attempted"
    /\ model_step_count' = model_step_count + 1
    /\ host_inserted' = TRUE
    /\ UnchangedFrame_fe440a7fd11975c1


RegisterHostHandleRestored ==
    /\ phase = "Restored"
    /\ phase' = "Restored"
    /\ model_step_count' = model_step_count + 1
    /\ host_inserted' = TRUE
    /\ UnchangedFrame_fe440a7fd11975c1


Next ==
    \/ ExpectCensusOwed(TRUE)
    \/ ExpectCensusOwedComplete(TRUE)
    \/ ExpectCensusNotOwed(FALSE)
    \/ ExpectCensusNotOwedHostInserts(FALSE)
    \/ (phase = "Attempted") /\ \E owed \in BOOLEAN : ExpectCensusAttempted(owed)
    \/ (phase = "Restored") /\ \E owed \in BOOLEAN : ExpectCensusRestored(owed)
    \/ BeginPassFirst
    \/ BeginPassNoCensusOwed
    \/ BeginPassAgain
    \/ BeginPassRestored
    \/ FinishPassCompletesCensus
    \/ FinishPassCensusComplete
    \/ FinishPassUnrestored
    \/ FinishPassRestored
    \/ DeclareRestoredUnrestored
    \/ DeclareRestoredUnrestoredComplete
    \/ DeclareRestoredAttempted
    \/ DeclareRestoredAttemptedComplete
    \/ DeclareRestoredRestored
    \/ RegisterHostHandleUnrestored
    \/ RegisterHostHandleUnrestoredComplete
    \/ RegisterHostHandleAttempted
    \/ RegisterHostHandleAttemptedComplete
    \/ RegisterHostHandleRestored

a_restored_set_has_its_census == (IF (phase # "Restored") THEN TRUE ELSE census_complete)
a_pass_is_begun_only_where_a_census_is_owed == (IF (phase = "Unrestored") THEN TRUE ELSE census_owed)
all_mobs_are_known_exactly_when_none_can_still_turn_up == (IF (all_mobs_known /\ (IF census_complete THEN TRUE ELSE (~(census_owed) /\ ~(host_inserted)))) THEN TRUE ELSE (~(all_mobs_known) /\ ~(census_complete) /\ (IF census_owed THEN TRUE ELSE host_inserted)))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []a_restored_set_has_its_census
THEOREM Spec => []a_pass_is_begun_only_where_a_census_is_owed
THEOREM Spec => []all_mobs_are_known_exactly_when_none_can_still_turn_up

=============================================================================
