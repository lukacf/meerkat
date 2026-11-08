---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for ToolGrantLifecycleMachine.

CONSTANTS NatValues, StringValues, ToolGrantEndReasonValues, ToolGrantIssueRefusalValues, ToolGrantRunIdValues, ToolGrantUseRefusalValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

ToolGrantRunIdValuesCi == {"alpha", "beta"}

ToolGrantRunIdValuesDeep == {"alpha", "beta"}

OptionToolGrantEndReasonValues == {None} \cup {Some(x) : x \in ToolGrantEndReasonValues}
OptionToolGrantRunIdValues == {None} \cup {Some(x) : x \in ToolGrantRunIdValues}
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

VARIABLES phase, model_step_count, grant_id, issued_at_ms, expires_at_ms, ended_at_ms, end_reason, run_scope

vars == << phase, model_step_count, grant_id, issued_at_ms, expires_at_ms, ended_at_ms, end_reason, run_scope >>

Init ==
    /\ phase = "Unissued"
    /\ model_step_count = 0
    /\ grant_id = ""
    /\ issued_at_ms = 0
    /\ expires_at_ms = 0
    /\ ended_at_ms = None
    /\ end_reason = None
    /\ run_scope = None

TerminalStutter ==
    /\ phase = "Expired" \/ phase = "Revoked" \/ phase = "RunEnded"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_94c0de1bac8ef303 == UNCHANGED << ended_at_ms, end_reason >>
UnchangedFrame_bf68d02069a0314e == UNCHANGED << grant_id, issued_at_ms, expires_at_ms, ended_at_ms, end_reason, run_scope >>
UnchangedFrame_e6c699a4a6dfbb07 == UNCHANGED << grant_id, issued_at_ms, expires_at_ms, run_scope >>

Issue(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope) ==
    /\ phase = "Unissued"
    /\ (arg_grant_id # "")
    /\ (now_ms < arg_expires_at_ms)
    /\ ((arg_expires_at_ms - now_ms) <= max_lifetime_ms)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ grant_id' = arg_grant_id
    /\ issued_at_ms' = now_ms
    /\ expires_at_ms' = arg_expires_at_ms
    /\ run_scope' = arg_run_scope
    /\ UnchangedFrame_94c0de1bac8ef303


IssueRefusedWindow(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope) ==
    /\ phase = "Unissued"
    /\ (IF (arg_expires_at_ms <= now_ms) THEN TRUE ELSE ((arg_expires_at_ms - now_ms) > max_lifetime_ms))
    /\ phase' = "Unissued"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


IssueRefusedIssuedActive(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope) ==
    /\ phase = "Active"
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


IssueRefusedIssuedExpired(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


IssueRefusedIssuedRevoked(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


ExpireDue(now_ms) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_e6c699a4a6dfbb07


ExpireNotDue(now_ms) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


ExpireSettledExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


ExpireSettledRevoked(now_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


RevokeActive(now_ms) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Revoked")
    /\ UnchangedFrame_e6c699a4a6dfbb07


RevokeAfterDeadline(now_ms) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_e6c699a4a6dfbb07


RevokeSettledExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


RevokeSettledRevoked(now_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


EndWithSubjectActive(now_ms) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("SubjectEnded")
    /\ UnchangedFrame_e6c699a4a6dfbb07


EndWithSubjectAfterDeadline(now_ms) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_e6c699a4a6dfbb07


EndWithSubjectSettledExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


EndWithSubjectSettledRevoked(now_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


AdmitLive(now_ms, run_id) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ (IF (run_scope = None) THEN TRUE ELSE ((run_id # None) /\ (run_scope = run_id)))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


AdmitDue(now_ms, run_id) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_e6c699a4a6dfbb07


AdmitEndedExpired(now_ms, run_id) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


AdmitEndedRevoked(now_ms, run_id) ==
    /\ phase = "Revoked"
    /\ (end_reason = Some("Revoked"))
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


AdmitEndedRunEnded(now_ms, run_id) ==
    /\ phase = "RunEnded"
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


AdmitOtherRun(now_ms, run_id) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ ((run_scope # None) /\ (run_id # None) /\ (run_scope # run_id))
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("RunEnded")
    /\ UnchangedFrame_e6c699a4a6dfbb07


AdmitOutsideRun(now_ms, run_id) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ ((run_scope # None) /\ (run_id = None))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


EndRunMatching(run_id, now_ms) ==
    /\ phase = "Active"
    /\ (now_ms < expires_at_ms)
    /\ (run_scope = Some(run_id))
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("RunEnded")
    /\ UnchangedFrame_e6c699a4a6dfbb07


EndRunAfterDeadline(run_id, now_ms) ==
    /\ phase = "Active"
    /\ (expires_at_ms <= now_ms)
    /\ (run_scope = Some(run_id))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ end_reason' = Some("Deadline")
    /\ UnchangedFrame_e6c699a4a6dfbb07


EndRunOther(run_id, now_ms) ==
    /\ phase = "Active"
    /\ (run_scope # Some(run_id))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


EndRunSettledExpired(run_id, now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


EndRunSettledRevoked(run_id, now_ms) ==
    /\ phase = "Revoked"
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


EndRunSettledRunEnded(run_id, now_ms) ==
    /\ phase = "RunEnded"
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


IssueRefusedIssuedRunEnded(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope) ==
    /\ phase = "RunEnded"
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


ExpireSettledRunEnded(now_ms) ==
    /\ phase = "RunEnded"
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


RevokeSettledRunEnded(now_ms) ==
    /\ phase = "RunEnded"
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


AdmitEndedWithSubject(now_ms, run_id) ==
    /\ phase = "Revoked"
    /\ (end_reason = Some("SubjectEnded"))
    /\ phase' = "Revoked"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


EndWithSubjectSettledRunEnded(now_ms) ==
    /\ phase = "RunEnded"
    /\ phase' = "RunEnded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_bf68d02069a0314e


Next ==
    \/ (phase = "Unissued") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : \E arg_run_scope \in OptionToolGrantRunIdValues : Issue(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope)
    \/ (phase = "Unissued") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : \E arg_run_scope \in OptionToolGrantRunIdValues : IssueRefusedWindow(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope)
    \/ (phase = "Active") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : \E arg_run_scope \in OptionToolGrantRunIdValues : IssueRefusedIssuedActive(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope)
    \/ (phase = "Expired") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : \E arg_run_scope \in OptionToolGrantRunIdValues : IssueRefusedIssuedExpired(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope)
    \/ (phase = "Revoked") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : \E arg_run_scope \in OptionToolGrantRunIdValues : IssueRefusedIssuedRevoked(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : ExpireDue(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : ExpireNotDue(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : ExpireSettledExpired(now_ms)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : ExpireSettledRevoked(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : RevokeActive(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : RevokeAfterDeadline(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : RevokeSettledExpired(now_ms)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : RevokeSettledRevoked(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : EndWithSubjectActive(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : EndWithSubjectAfterDeadline(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : EndWithSubjectSettledExpired(now_ms)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : EndWithSubjectSettledRevoked(now_ms)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitLive(now_ms, run_id)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitDue(now_ms, run_id)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitEndedExpired(now_ms, run_id)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitEndedRevoked(now_ms, run_id)
    \/ (phase = "RunEnded") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitEndedRunEnded(now_ms, run_id)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitOtherRun(now_ms, run_id)
    \/ (phase = "Active") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitOutsideRun(now_ms, run_id)
    \/ (phase = "Active") /\ \E run_id \in ToolGrantRunIdValues : \E now_ms \in 0..2 : EndRunMatching(run_id, now_ms)
    \/ (phase = "Active") /\ \E run_id \in ToolGrantRunIdValues : \E now_ms \in 0..2 : EndRunAfterDeadline(run_id, now_ms)
    \/ (phase = "Active") /\ \E run_id \in ToolGrantRunIdValues : \E now_ms \in 0..2 : EndRunOther(run_id, now_ms)
    \/ (phase = "Expired") /\ \E run_id \in ToolGrantRunIdValues : \E now_ms \in 0..2 : EndRunSettledExpired(run_id, now_ms)
    \/ (phase = "Revoked") /\ \E run_id \in ToolGrantRunIdValues : \E now_ms \in 0..2 : EndRunSettledRevoked(run_id, now_ms)
    \/ (phase = "RunEnded") /\ \E run_id \in ToolGrantRunIdValues : \E now_ms \in 0..2 : EndRunSettledRunEnded(run_id, now_ms)
    \/ (phase = "RunEnded") /\ \E arg_grant_id \in StringValues : \E now_ms \in 0..2 : \E arg_expires_at_ms \in 0..2 : \E max_lifetime_ms \in 0..2 : \E arg_run_scope \in OptionToolGrantRunIdValues : IssueRefusedIssuedRunEnded(arg_grant_id, now_ms, arg_expires_at_ms, max_lifetime_ms, arg_run_scope)
    \/ (phase = "RunEnded") /\ \E now_ms \in 0..2 : ExpireSettledRunEnded(now_ms)
    \/ (phase = "RunEnded") /\ \E now_ms \in 0..2 : RevokeSettledRunEnded(now_ms)
    \/ (phase = "Revoked") /\ \E now_ms \in 0..2 : \E run_id \in OptionToolGrantRunIdValues : AdmitEndedWithSubject(now_ms, run_id)
    \/ (phase = "RunEnded") /\ \E now_ms \in 0..2 : EndWithSubjectSettledRunEnded(now_ms)
    \/ TerminalStutter

unissued_is_empty == (IF (phase # "Unissued") THEN TRUE ELSE ((grant_id = "") /\ (issued_at_ms = 0) /\ (expires_at_ms = 0) /\ (ended_at_ms = None) /\ (end_reason = None)))
issued_window_is_nonempty == (IF (phase = "Unissued") THEN TRUE ELSE ((grant_id # "") /\ (issued_at_ms < expires_at_ms)))
active_has_no_end == (IF (phase # "Active") THEN TRUE ELSE ((ended_at_ms = None) /\ (end_reason = None)))
ended_grants_record_their_end == (IF ((phase # "Expired") /\ (phase # "Revoked") /\ (phase # "RunEnded")) THEN TRUE ELSE ((ended_at_ms # None) /\ (end_reason # None)))
expiry_never_precedes_its_deadline == (IF (phase # "Expired") THEN TRUE ELSE ((end_reason = Some("Deadline")) /\ (ended_at_ms # None) /\ ((IF "value" \in DOMAIN ended_at_ms THEN ended_at_ms["value"] ELSE None) >= expires_at_ms)))
revocation_precedes_the_deadline == (IF (phase # "Revoked") THEN TRUE ELSE ((IF (end_reason = Some("Revoked")) THEN TRUE ELSE (end_reason = Some("SubjectEnded"))) /\ (ended_at_ms # None) /\ ((IF "value" \in DOMAIN ended_at_ms THEN ended_at_ms["value"] ELSE None) < expires_at_ms)))
run_ended_records_its_end == (IF (phase # "RunEnded") THEN TRUE ELSE ((end_reason = Some("RunEnded")) /\ (ended_at_ms # None) /\ ((IF "value" \in DOMAIN ended_at_ms THEN ended_at_ms["value"] ELSE None) < expires_at_ms) /\ (run_scope # None)))
run_scope_set_only_by_issue == (IF (phase # "Unissued") THEN TRUE ELSE (run_scope = None))

CiStateConstraint == /\ model_step_count <= 4
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []unissued_is_empty
THEOREM Spec => []issued_window_is_nonempty
THEOREM Spec => []active_has_no_end
THEOREM Spec => []ended_grants_record_their_end
THEOREM Spec => []expiry_never_precedes_its_deadline
THEOREM Spec => []revocation_precedes_the_deadline
THEOREM Spec => []run_ended_records_its_end
THEOREM Spec => []run_scope_set_only_by_issue

=============================================================================
