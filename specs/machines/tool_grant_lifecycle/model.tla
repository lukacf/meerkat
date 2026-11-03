---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ToolGrantLifecycleMachine.

CONSTANTS NatValues, StringValues, ToolGrantEndReasonValues, ToolGrantIssueRefusalValues, ToolGrantUseRefusalValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionToolGrantEndReasonValues == {None} \cup {Some(x) : x \in ToolGrantEndReasonValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

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

VARIABLES phase, model_step_count, grant_id, issued_at_ms, expires_at_ms, ended_at_ms, end_reason

vars == << phase, model_step_count, grant_id, issued_at_ms, expires_at_ms, ended_at_ms, end_reason >>

Init ==
    /\ phase = "Unissued"
    /\ model_step_count = 0
    /\ grant_id = ""
    /\ issued_at_ms = 0
    /\ expires_at_ms = 0
    /\ ended_at_ms = None
    /\ end_reason = None

TerminalStutter ==
    /\ phase = "Expired" \/ phase = "Revoked"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_879ce4ccada6bac3 == UNCHANGED << grant_id, issued_at_ms, expires_at_ms >>
UnchangedFrame_94c0de1bac8ef303 == UNCHANGED << ended_at_ms, end_reason >>
UnchangedFrame_99683cc0d992b69c == UNCHANGED << grant_id, issued_at_ms, expires_at_ms, ended_at_ms, end_reason >>

Issue(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms) ==
    /\ phase = "Unissued"
    /\ (arg_grant_id # "")
    /\ (now_ms < arg_expires_at_ms)
    /\ ((arg_expires_at_ms - now_ms) <= max_lifetime_ms)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ grant_id' = arg_grant_id
    /\ issued_at_ms' = now_ms
    /\ expires_at_ms' = arg_expires_at_ms
    /\ UnchangedFrame_94c0de1bac8ef303


IssueRefusedWindow(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms) ==
    /\ phase = "Unissued"
    /\ (IF (arg_expires_at_ms <= now_ms) THEN TRUE ELSE ((arg_expires_at_ms - now_ms) > max_lifetime_ms))
    /\ phase' = "Unissued"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


IssueRefusedIssuedActive(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


IssueRefusedIssuedExpired(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


IssueRefusedIssuedRevoked(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


ExpireDue(now_ms) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_879ce4ccada6bac3


ExpireNotDue(now_ms) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


ExpireSettledExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


ExpireSettledRevoked(now_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


RevokeActive(now_ms) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Revoked")
    /\ UnchangedFrame_879ce4ccada6bac3


RevokeAfterDeadline(now_ms) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_879ce4ccada6bac3


RevokeSettledExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


RevokeSettledRevoked(now_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


AdmitLive(now_ms) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


AdmitDue(now_ms) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_879ce4ccada6bac3


AdmitEndedExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


AdmitEndedRevoked(now_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_99683cc0d992b69c


Next ==
    \/ (phase = "Unissued") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : Issue(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms)
    \/ (phase = "Unissued") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : IssueRefusedWindow(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms)
    \/ (phase = "Active") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : IssueRefusedIssuedActive(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms)
    \/ (phase = "Expired") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : IssueRefusedIssuedExpired(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms)
    \/ (phase = "Revoked") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : IssueRefusedIssuedRevoked(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : ExpireDue(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : ExpireNotDue(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : ExpireSettledExpired(now_ms)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : ExpireSettledRevoked(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : RevokeActive(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : RevokeAfterDeadline(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : RevokeSettledExpired(now_ms)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : RevokeSettledRevoked(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : AdmitLive(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : AdmitDue(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : AdmitEndedExpired(now_ms)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : AdmitEndedRevoked(now_ms)
    \/ TerminalStutter

unissued_is_empty == (IF (phase # "Unissued") THEN TRUE ELSE ((grant_id = "") /\ (issued_at_ms = 0) /\ (expires_at_ms = 0) /\ (ended_at_ms = None) /\ (end_reason = None)))
issued_window_is_nonempty == (IF (phase = "Unissued") THEN TRUE ELSE ((grant_id # "") /\ (issued_at_ms < expires_at_ms)))
active_has_no_end == (IF (phase # "Active") THEN TRUE ELSE ((ended_at_ms = None) /\ (end_reason = None)))
ended_grants_record_their_end == (IF ((phase # "Expired") /\ (phase # "Revoked")) THEN TRUE ELSE ((ended_at_ms # None) /\ (end_reason # None)))
expiry_never_precedes_its_deadline == (IF (phase # "Expired") THEN TRUE ELSE ((end_reason = Some("Deadline")) /\ (ended_at_ms # None) /\ ((IF "value" \in DOMAIN ended_at_ms THEN ended_at_ms["value"] ELSE None) >= expires_at_ms)))
revocation_precedes_the_deadline == (IF (phase # "Revoked") THEN TRUE ELSE ((end_reason = Some("Revoked")) /\ (ended_at_ms # None) /\ ((IF "value" \in DOMAIN ended_at_ms THEN ended_at_ms["value"] ELSE None) < expires_at_ms)))

CiStateConstraint == /\ model_step_count <= 4
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []unissued_is_empty
THEOREM Spec => []issued_window_is_nonempty
THEOREM Spec => []active_has_no_end
THEOREM Spec => []ended_grants_record_their_end
THEOREM Spec => []expiry_never_precedes_its_deadline
THEOREM Spec => []revocation_precedes_the_deadline

=============================================================================
