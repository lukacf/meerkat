---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for DurableDecisionMachine.

CONSTANTS BooleanValues, DecisionDigestValues, DecisionPartyValues, DurableDecisionNoticeValues, DurableDecisionRefusalValues, DurableDecisionReviewVerdictValues, DurableDecisionStageValues, NatValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

DecisionDigestValuesCi == {"alpha", "beta"}
DecisionPartyValuesCi == {"alpha", "beta"}

DecisionDigestValuesDeep == {"alpha", "beta"}
DecisionPartyValuesDeep == {"alpha", "beta"}

OptionDecisionDigestValues == {None} \cup {Some(x) : x \in DecisionDigestValues}
OptionDecisionPartyValues == {None} \cup {Some(x) : x \in DecisionPartyValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
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

VARIABLES phase, model_step_count, decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest, notice

vars == << phase, model_step_count, decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest, notice >>

earliest_presentation(first, presented) == (IF ((presented # None) /\ (IF (first = None) THEN TRUE ELSE ((IF "value" \in DOMAIN presented THEN presented["value"] ELSE None) < (IF "value" \in DOMAIN first THEN first["value"] ELSE None)))) THEN presented ELSE first)
owed_if_due(arg_notice) == (IF (arg_notice = "NotDue") THEN "Owed" ELSE arg_notice)

Init ==
    /\ phase = "Unheld"
    /\ model_step_count = 0
    /\ decision_id = ""
    /\ deadline_at_ms = 0
    /\ subject_digest = ""
    /\ operation_id = ""
    /\ request_digest = ""
    /\ requester = ""
    /\ approver_id = ""
    /\ reviewer = None
    /\ stage = "Human"
    /\ first_presented_to_approver_at_ms = None
    /\ presented_to_reviewer = FALSE
    /\ evidence_digest = None
    /\ decided_at_ms = None
    /\ ended_at_ms = None
    /\ permit_issued = FALSE
    /\ executed_request_digest = None
    /\ notice = "NotRequired"

TerminalStutter ==
    /\ phase = "Rejected" \/ phase = "Expired" \/ phase = "Withdrawn" \/ phase = "Executed" \/ phase = "ExecutionRefused"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_041dbd55facc8e9e == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, executed_request_digest >>
UnchangedFrame_081ec0a790bd4234 == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest, notice >>
UnchangedFrame_28c3b877b20b98cc == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest, notice >>
UnchangedFrame_5655776319fa263a == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, permit_issued, executed_request_digest >>
UnchangedFrame_59514c21e9acc6fa == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms >>
UnchangedFrame_5d0bcc6d5b0cedfc == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest >>
UnchangedFrame_8f0e447ef7c4d8f2 == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, presented_to_reviewer, permit_issued, executed_request_digest >>
UnchangedFrame_8fe652afe0986773 == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, ended_at_ms, permit_issued, executed_request_digest, notice >>
UnchangedFrame_c0495e5e907ed933 == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, executed_request_digest, notice >>
UnchangedFrame_c9db057c1a9d976b == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, presented_to_reviewer, ended_at_ms, permit_issued, executed_request_digest, notice >>
UnchangedFrame_ddf2e79855276aa2 == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest, notice >>
UnchangedFrame_e45449ec14b3ab35 == UNCHANGED << first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest >>
UnchangedFrame_e951506f84a53247 == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, permit_issued, executed_request_digest >>
UnchangedFrame_f7d4c4b3385291f4 == UNCHANGED << decision_id, deadline_at_ms, subject_digest, operation_id, request_digest, requester, approver_id, reviewer, stage, first_presented_to_approver_at_ms, presented_to_reviewer, evidence_digest, decided_at_ms, ended_at_ms, permit_issued, executed_request_digest, notice >>

HoldHuman(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Unheld"
    /\ (arg_decision_id # "")
    /\ (arg_operation_id # "")
    /\ (now_ms < arg_deadline_at_ms)
    /\ (arg_reviewer = None)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ decision_id' = arg_decision_id
    /\ deadline_at_ms' = arg_deadline_at_ms
    /\ subject_digest' = arg_subject_digest
    /\ operation_id' = arg_operation_id
    /\ request_digest' = arg_request_digest
    /\ requester' = arg_requester
    /\ approver_id' = arg_approver_id
    /\ reviewer' = arg_reviewer
    /\ stage' = "Human"
    /\ notice' = IF notice_routable THEN "NotDue" ELSE notice
    /\ UnchangedFrame_e45449ec14b3ab35


HoldReview(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Unheld"
    /\ (arg_decision_id # "")
    /\ (arg_operation_id # "")
    /\ (now_ms < arg_deadline_at_ms)
    /\ (arg_reviewer # None)
    /\ (arg_reviewer # Some(arg_requester))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ decision_id' = arg_decision_id
    /\ deadline_at_ms' = arg_deadline_at_ms
    /\ subject_digest' = arg_subject_digest
    /\ operation_id' = arg_operation_id
    /\ request_digest' = arg_request_digest
    /\ requester' = arg_requester
    /\ approver_id' = arg_approver_id
    /\ reviewer' = arg_reviewer
    /\ stage' = "Review"
    /\ notice' = IF notice_routable THEN "NotDue" ELSE notice
    /\ UnchangedFrame_e45449ec14b3ab35


HoldRefusedWindow(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Unheld"
    /\ (arg_deadline_at_ms <= now_ms)
    /\ phase' = "Unheld"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldRefusedSelfReview(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Unheld"
    /\ (now_ms < arg_deadline_at_ms)
    /\ (arg_reviewer = Some(arg_requester))
    /\ phase' = "Unheld"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldReplayedOpen(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Open"
    /\ (arg_decision_id = decision_id)
    /\ (arg_deadline_at_ms = deadline_at_ms)
    /\ (arg_subject_digest = subject_digest)
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (arg_requester = requester)
    /\ (arg_approver_id = approver_id)
    /\ (arg_reviewer = reviewer)
    /\ (notice_routable = (notice # "NotRequired"))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldReplayedApproved(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Approved"
    /\ (arg_decision_id = decision_id)
    /\ (arg_deadline_at_ms = deadline_at_ms)
    /\ (arg_subject_digest = subject_digest)
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (arg_requester = requester)
    /\ (arg_approver_id = approver_id)
    /\ (arg_reviewer = reviewer)
    /\ (notice_routable = (notice # "NotRequired"))
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldReplayedRejected(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Rejected"
    /\ (arg_decision_id = decision_id)
    /\ (arg_deadline_at_ms = deadline_at_ms)
    /\ (arg_subject_digest = subject_digest)
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (arg_requester = requester)
    /\ (arg_approver_id = approver_id)
    /\ (arg_reviewer = reviewer)
    /\ (notice_routable = (notice # "NotRequired"))
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldReplayedExpired(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Expired"
    /\ (arg_decision_id = decision_id)
    /\ (arg_deadline_at_ms = deadline_at_ms)
    /\ (arg_subject_digest = subject_digest)
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (arg_requester = requester)
    /\ (arg_approver_id = approver_id)
    /\ (arg_reviewer = reviewer)
    /\ (notice_routable = (notice # "NotRequired"))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldReplayedWithdrawn(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Withdrawn"
    /\ (arg_decision_id = decision_id)
    /\ (arg_deadline_at_ms = deadline_at_ms)
    /\ (arg_subject_digest = subject_digest)
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (arg_requester = requester)
    /\ (arg_approver_id = approver_id)
    /\ (arg_reviewer = reviewer)
    /\ (notice_routable = (notice # "NotRequired"))
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldReplayedExecuted(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Executed"
    /\ (arg_decision_id = decision_id)
    /\ (arg_deadline_at_ms = deadline_at_ms)
    /\ (arg_subject_digest = subject_digest)
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (arg_requester = requester)
    /\ (arg_approver_id = approver_id)
    /\ (arg_reviewer = reviewer)
    /\ (notice_routable = (notice # "NotRequired"))
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldReplayedExecutionRefused(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "ExecutionRefused"
    /\ (arg_decision_id = decision_id)
    /\ (arg_deadline_at_ms = deadline_at_ms)
    /\ (arg_subject_digest = subject_digest)
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (arg_requester = requester)
    /\ (arg_approver_id = approver_id)
    /\ (arg_reviewer = reviewer)
    /\ (notice_routable = (notice # "NotRequired"))
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldConflictOpen(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Open"
    /\ (IF (arg_decision_id # decision_id) THEN TRUE ELSE (IF (arg_deadline_at_ms # deadline_at_ms) THEN TRUE ELSE (IF (arg_subject_digest # subject_digest) THEN TRUE ELSE (IF (arg_operation_id # operation_id) THEN TRUE ELSE (IF (arg_request_digest # request_digest) THEN TRUE ELSE (IF (arg_requester # requester) THEN TRUE ELSE (IF (arg_approver_id # approver_id) THEN TRUE ELSE (IF (arg_reviewer # reviewer) THEN TRUE ELSE (notice_routable # (notice # "NotRequired"))))))))))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldConflictApproved(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Approved"
    /\ (IF (arg_decision_id # decision_id) THEN TRUE ELSE (IF (arg_deadline_at_ms # deadline_at_ms) THEN TRUE ELSE (IF (arg_subject_digest # subject_digest) THEN TRUE ELSE (IF (arg_operation_id # operation_id) THEN TRUE ELSE (IF (arg_request_digest # request_digest) THEN TRUE ELSE (IF (arg_requester # requester) THEN TRUE ELSE (IF (arg_approver_id # approver_id) THEN TRUE ELSE (IF (arg_reviewer # reviewer) THEN TRUE ELSE (notice_routable # (notice # "NotRequired"))))))))))
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldConflictRejected(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Rejected"
    /\ (IF (arg_decision_id # decision_id) THEN TRUE ELSE (IF (arg_deadline_at_ms # deadline_at_ms) THEN TRUE ELSE (IF (arg_subject_digest # subject_digest) THEN TRUE ELSE (IF (arg_operation_id # operation_id) THEN TRUE ELSE (IF (arg_request_digest # request_digest) THEN TRUE ELSE (IF (arg_requester # requester) THEN TRUE ELSE (IF (arg_approver_id # approver_id) THEN TRUE ELSE (IF (arg_reviewer # reviewer) THEN TRUE ELSE (notice_routable # (notice # "NotRequired"))))))))))
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldConflictExpired(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Expired"
    /\ (IF (arg_decision_id # decision_id) THEN TRUE ELSE (IF (arg_deadline_at_ms # deadline_at_ms) THEN TRUE ELSE (IF (arg_subject_digest # subject_digest) THEN TRUE ELSE (IF (arg_operation_id # operation_id) THEN TRUE ELSE (IF (arg_request_digest # request_digest) THEN TRUE ELSE (IF (arg_requester # requester) THEN TRUE ELSE (IF (arg_approver_id # approver_id) THEN TRUE ELSE (IF (arg_reviewer # reviewer) THEN TRUE ELSE (notice_routable # (notice # "NotRequired"))))))))))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldConflictWithdrawn(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Withdrawn"
    /\ (IF (arg_decision_id # decision_id) THEN TRUE ELSE (IF (arg_deadline_at_ms # deadline_at_ms) THEN TRUE ELSE (IF (arg_subject_digest # subject_digest) THEN TRUE ELSE (IF (arg_operation_id # operation_id) THEN TRUE ELSE (IF (arg_request_digest # request_digest) THEN TRUE ELSE (IF (arg_requester # requester) THEN TRUE ELSE (IF (arg_approver_id # approver_id) THEN TRUE ELSE (IF (arg_reviewer # reviewer) THEN TRUE ELSE (notice_routable # (notice # "NotRequired"))))))))))
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldConflictExecuted(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "Executed"
    /\ (IF (arg_decision_id # decision_id) THEN TRUE ELSE (IF (arg_deadline_at_ms # deadline_at_ms) THEN TRUE ELSE (IF (arg_subject_digest # subject_digest) THEN TRUE ELSE (IF (arg_operation_id # operation_id) THEN TRUE ELSE (IF (arg_request_digest # request_digest) THEN TRUE ELSE (IF (arg_requester # requester) THEN TRUE ELSE (IF (arg_approver_id # approver_id) THEN TRUE ELSE (IF (arg_reviewer # reviewer) THEN TRUE ELSE (notice_routable # (notice # "NotRequired"))))))))))
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


HoldConflictExecutionRefused(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable) ==
    /\ phase = "ExecutionRefused"
    /\ (IF (arg_decision_id # decision_id) THEN TRUE ELSE (IF (arg_deadline_at_ms # deadline_at_ms) THEN TRUE ELSE (IF (arg_subject_digest # subject_digest) THEN TRUE ELSE (IF (arg_operation_id # operation_id) THEN TRUE ELSE (IF (arg_request_digest # request_digest) THEN TRUE ELSE (IF (arg_requester # requester) THEN TRUE ELSE (IF (arg_approver_id # approver_id) THEN TRUE ELSE (IF (arg_reviewer # reviewer) THEN TRUE ELSE (notice_routable # (notice # "NotRequired"))))))))))
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentAtDeadline(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (deadline_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


PresentRefusedSubjectChanged(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (digest # subject_digest)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedIncomplete(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (digest = subject_digest)
    /\ ~(complete)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentToApprover(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (digest = subject_digest)
    /\ complete
    /\ (stage = "Human")
    /\ (audience = approver_id)
    /\ (presented_at_ms # 0)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ first_presented_to_approver_at_ms' = earliest_presentation(first_presented_to_approver_at_ms, Some(presented_at_ms))
    /\ UnchangedFrame_28c3b877b20b98cc


PresentRefusedUntimed(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (digest = subject_digest)
    /\ complete
    /\ (stage = "Human")
    /\ (audience = approver_id)
    /\ (presented_at_ms = 0)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedWrongApproverAudience(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (digest = subject_digest)
    /\ complete
    /\ (stage = "Human")
    /\ (audience # approver_id)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentToReviewer(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (digest = subject_digest)
    /\ complete
    /\ (stage = "Review")
    /\ (reviewer = Some(audience))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ presented_to_reviewer' = TRUE
    /\ UnchangedFrame_ddf2e79855276aa2


PresentRefusedWrongReviewerAudience(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (digest = subject_digest)
    /\ complete
    /\ (stage = "Review")
    /\ (reviewer # Some(audience))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedNotOpenApproved(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Approved"
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedNotOpenRejected(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedNotOpenExpired(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedNotOpenWithdrawn(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedNotOpenExecuted(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


PresentRefusedNotOpenExecutionRefused(audience, digest, complete, presented_at_ms, now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideAtDeadline(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (deadline_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


DecideRefusedReviewStage(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedEvidenceRequired(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest = None)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedSubjectChanged(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest # None)
    /\ (digest # subject_digest)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedNotEligible(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ (IF (arg_approver_id # approver_id) THEN TRUE ELSE ~(eligible))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedSelfApproval(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ (arg_approver_id = approver_id)
    /\ eligible
    /\ (decider = requester)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedPresentationIncomplete(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ (arg_approver_id = approver_id)
    /\ eligible
    /\ (decider # requester)
    /\ (IF ((carried_presentation_at_ms = None) /\ (first_presented_to_approver_at_ms = None)) THEN TRUE ELSE (carried_presentation_at_ms = Some(0)))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedDecidedBeforePresentation(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ (arg_approver_id = approver_id)
    /\ eligible
    /\ (decider # requester)
    /\ (carried_presentation_at_ms # Some(0))
    /\ (IF (carried_presentation_at_ms # None) THEN TRUE ELSE (first_presented_to_approver_at_ms # None))
    /\ (IF (first_presented_to_approver_at_ms = None) THEN TRUE ELSE (observed_at_ms < (IF "value" \in DOMAIN first_presented_to_approver_at_ms THEN first_presented_to_approver_at_ms["value"] ELSE None)))
    /\ (IF (carried_presentation_at_ms = None) THEN TRUE ELSE (observed_at_ms < (IF "value" \in DOMAIN carried_presentation_at_ms THEN carried_presentation_at_ms["value"] ELSE None)))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideApproved(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ (arg_approver_id = approver_id)
    /\ eligible
    /\ (decider # requester)
    /\ (carried_presentation_at_ms # Some(0))
    /\ (IF ((first_presented_to_approver_at_ms # None) /\ ((IF "value" \in DOMAIN first_presented_to_approver_at_ms THEN first_presented_to_approver_at_ms["value"] ELSE None) <= observed_at_ms)) THEN TRUE ELSE ((carried_presentation_at_ms # None) /\ ((IF "value" \in DOMAIN carried_presentation_at_ms THEN carried_presentation_at_ms["value"] ELSE None) <= observed_at_ms)))
    /\ approve
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ first_presented_to_approver_at_ms' = earliest_presentation(first_presented_to_approver_at_ms, carried_presentation_at_ms)
    /\ evidence_digest' = arg_evidence_digest
    /\ decided_at_ms' = Some(now_ms)
    /\ UnchangedFrame_c9db057c1a9d976b


DecideRejected(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ (arg_approver_id = approver_id)
    /\ eligible
    /\ (decider # requester)
    /\ (carried_presentation_at_ms # Some(0))
    /\ (IF ((first_presented_to_approver_at_ms # None) /\ ((IF "value" \in DOMAIN first_presented_to_approver_at_ms THEN first_presented_to_approver_at_ms["value"] ELSE None) <= observed_at_ms)) THEN TRUE ELSE ((carried_presentation_at_ms # None) /\ ((IF "value" \in DOMAIN carried_presentation_at_ms THEN carried_presentation_at_ms["value"] ELSE None) <= observed_at_ms)))
    /\ ~(approve)
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ first_presented_to_approver_at_ms' = earliest_presentation(first_presented_to_approver_at_ms, carried_presentation_at_ms)
    /\ evidence_digest' = arg_evidence_digest
    /\ decided_at_ms' = Some(now_ms)
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_8f0e447ef7c4d8f2


DecideRefusedNotOpenApproved(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Approved"
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedNotOpenRejected(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedNotOpenExpired(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedNotOpenWithdrawn(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedNotOpenExecuted(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


DecideRefusedNotOpenExecutionRefused(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewAtDeadline(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (deadline_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


ReviewRefusedHumanStage(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Human")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedWrongReviewer(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ (reviewer # Some(arg_reviewer))
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedEvidenceRequired(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ (reviewer = Some(arg_reviewer))
    /\ (arg_evidence_digest = None)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedSubjectChanged(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ (reviewer = Some(arg_reviewer))
    /\ (arg_evidence_digest # None)
    /\ (digest # subject_digest)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedPresentationIncomplete(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ (reviewer = Some(arg_reviewer))
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ ~(presented_to_reviewer)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewAllowed(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ (reviewer = Some(arg_reviewer))
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ presented_to_reviewer
    /\ (verdict = "Allow")
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ evidence_digest' = arg_evidence_digest
    /\ decided_at_ms' = Some(now_ms)
    /\ UnchangedFrame_8fe652afe0986773


ReviewDenied(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ (reviewer = Some(arg_reviewer))
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ presented_to_reviewer
    /\ (verdict = "Deny")
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ evidence_digest' = arg_evidence_digest
    /\ decided_at_ms' = Some(now_ms)
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_5655776319fa263a


ReviewEscalated(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ (stage = "Review")
    /\ (reviewer = Some(arg_reviewer))
    /\ (arg_evidence_digest # None)
    /\ (digest = subject_digest)
    /\ presented_to_reviewer
    /\ (verdict = "Escalate")
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ stage' = "Human"
    /\ UnchangedFrame_081ec0a790bd4234


ReviewRefusedNotOpenApproved(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Approved"
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedNotOpenRejected(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedNotOpenExpired(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedNotOpenWithdrawn(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedNotOpenExecuted(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReviewRefusedNotOpenExecutionRefused(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


WithdrawAtDeadline(withdrawer, by_host, now_ms) ==
    /\ phase = "Open"
    /\ (deadline_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


WithdrawByHost(withdrawer, by_host, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ by_host
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


WithdrawByRequester(withdrawer, by_host, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ ~(by_host)
    /\ (withdrawer = requester)
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


WithdrawRefusedNotRequester(withdrawer, by_host, now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ ~(by_host)
    /\ (withdrawer # requester)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


WithdrawRefusedNotOpenApproved(withdrawer, by_host, now_ms) ==
    /\ phase = "Approved"
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


WithdrawRefusedNotOpenRejected(withdrawer, by_host, now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


WithdrawRefusedNotOpenExpired(withdrawer, by_host, now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


WithdrawRefusedNotOpenWithdrawn(withdrawer, by_host, now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


WithdrawRefusedNotOpenExecuted(withdrawer, by_host, now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


WithdrawRefusedNotOpenExecutionRefused(withdrawer, by_host, now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireOpenDue(now_ms) ==
    /\ phase = "Open"
    /\ (deadline_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


ExpireOpenNotDue(now_ms) ==
    /\ phase = "Open"
    /\ (now_ms < deadline_at_ms)
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireApprovedDue(now_ms) ==
    /\ phase = "Approved"
    /\ (deadline_at_ms <= now_ms)
    /\ ~(permit_issued)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


ExpireApprovedSettlementOwed(now_ms) ==
    /\ phase = "Approved"
    /\ (deadline_at_ms <= now_ms)
    /\ permit_issued
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireApprovedNotDue(now_ms) ==
    /\ phase = "Approved"
    /\ (now_ms < deadline_at_ms)
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireSettledRejected(now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireSettledExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireSettledWithdrawn(now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireSettledExecuted(now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ExpireSettledExecutionRefused(now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecution(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Approved"
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (now_ms < deadline_at_ms)
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ permit_issued' = TRUE
    /\ UnchangedFrame_c0495e5e907ed933


AuthorizeExecutionAtDeadline(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Approved"
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (deadline_at_ms <= now_ms)
    /\ ~(permit_issued)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


AuthorizeExecutionRefusedSettlementOwed(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Approved"
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ (deadline_at_ms <= now_ms)
    /\ permit_issued
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecutionRefusedBinding(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Approved"
    /\ (IF (arg_operation_id # operation_id) THEN TRUE ELSE (arg_request_digest # request_digest))
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecutionRefusedNotApprovedOpen(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecutionRefusedNotApprovedRejected(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecutionRefusedNotApprovedExpired(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecutionRefusedNotApprovedWithdrawn(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecutionRefusedNotApprovedExecuted(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


AuthorizeExecutionRefusedNotApprovedExecutionRefused(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReleaseExecution(now_ms) ==
    /\ phase = "Approved"
    /\ permit_issued
    /\ (now_ms < deadline_at_ms)
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ permit_issued' = FALSE
    /\ UnchangedFrame_c0495e5e907ed933


ReleaseExecutionAtDeadline(now_ms) ==
    /\ phase = "Approved"
    /\ permit_issued
    /\ (deadline_at_ms <= now_ms)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ permit_issued' = FALSE
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_041dbd55facc8e9e


ReleaseExecutionRefusedNotAuthorized(now_ms) ==
    /\ phase = "Approved"
    /\ ~(permit_issued)
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReleaseExecutionRefusedNotApprovedOpen(now_ms) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReleaseExecutionRefusedNotApprovedRejected(now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReleaseExecutionRefusedNotApprovedExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReleaseExecutionRefusedNotApprovedWithdrawn(now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReleaseExecutionRefusedNotApprovedExecuted(now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


ReleaseExecutionRefusedNotApprovedExecutionRefused(now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecuted(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Approved"
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ permit_issued
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ permit_issued' = FALSE
    /\ executed_request_digest' = Some(arg_request_digest)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_59514c21e9acc6fa


RecordExecutedRefusedNotAuthorized(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Approved"
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ ~(permit_issued)
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedRefusedBinding(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Approved"
    /\ (IF (arg_operation_id # operation_id) THEN TRUE ELSE (arg_request_digest # request_digest))
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedReplayed(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Executed"
    /\ (arg_operation_id = operation_id)
    /\ (arg_request_digest = request_digest)
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedReplayRefusedBinding(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Executed"
    /\ (IF (arg_operation_id # operation_id) THEN TRUE ELSE (arg_request_digest # request_digest))
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedRefusedNotApprovedOpen(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedRefusedNotApprovedRejected(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedRefusedNotApprovedExpired(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedRefusedNotApprovedWithdrawn(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


RecordExecutedRefusedNotApprovedExecutionRefused(arg_operation_id, arg_request_digest, now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


CloseRefused(now_ms) ==
    /\ phase = "Approved"
    /\ (now_ms < deadline_at_ms)
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ permit_issued' = FALSE
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_041dbd55facc8e9e


CloseRefusedWithPermit(now_ms) ==
    /\ phase = "Approved"
    /\ (deadline_at_ms <= now_ms)
    /\ permit_issued
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ permit_issued' = FALSE
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_041dbd55facc8e9e


CloseRefusedAtDeadline(now_ms) ==
    /\ phase = "Approved"
    /\ (deadline_at_ms <= now_ms)
    /\ ~(permit_issued)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ ended_at_ms' = Some(now_ms)
    /\ notice' = owed_if_due(notice)
    /\ UnchangedFrame_e951506f84a53247


CloseRefusedNotApprovedOpen(now_ms) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


CloseRefusedNotApprovedRejected(now_ms) ==
    /\ phase = "Rejected"
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


CloseRefusedNotApprovedExpired(now_ms) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


CloseRefusedNotApprovedWithdrawn(now_ms) ==
    /\ phase = "Withdrawn"
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


CloseRefusedNotApprovedExecuted(now_ms) ==
    /\ phase = "Executed"
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


CloseRefusedNotApprovedExecutionRefused(now_ms) ==
    /\ phase = "ExecutionRefused"
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeDeliveredRejected(delivered) ==
    /\ phase = "Rejected"
    /\ (notice = "Owed")
    /\ delivered
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Delivered"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeDeliveredExpired(delivered) ==
    /\ phase = "Expired"
    /\ (notice = "Owed")
    /\ delivered
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Delivered"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeDeliveredWithdrawn(delivered) ==
    /\ phase = "Withdrawn"
    /\ (notice = "Owed")
    /\ delivered
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Delivered"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeDeliveredExecuted(delivered) ==
    /\ phase = "Executed"
    /\ (notice = "Owed")
    /\ delivered
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Delivered"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeDeliveredExecutionRefused(delivered) ==
    /\ phase = "ExecutionRefused"
    /\ (notice = "Owed")
    /\ delivered
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Delivered"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeFailedRejected(delivered) ==
    /\ phase = "Rejected"
    /\ (notice = "Owed")
    /\ ~(delivered)
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Failed"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeFailedExpired(delivered) ==
    /\ phase = "Expired"
    /\ (notice = "Owed")
    /\ ~(delivered)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Failed"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeFailedWithdrawn(delivered) ==
    /\ phase = "Withdrawn"
    /\ (notice = "Owed")
    /\ ~(delivered)
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Failed"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeFailedExecuted(delivered) ==
    /\ phase = "Executed"
    /\ (notice = "Owed")
    /\ ~(delivered)
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Failed"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeFailedExecutionRefused(delivered) ==
    /\ phase = "ExecutionRefused"
    /\ (notice = "Owed")
    /\ ~(delivered)
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ notice' = "Failed"
    /\ UnchangedFrame_5d0bcc6d5b0cedfc


SettleNoticeReplayedRejected(delivered) ==
    /\ phase = "Rejected"
    /\ (IF ((notice = "Delivered") /\ delivered) THEN TRUE ELSE ((notice = "Failed") /\ ~(delivered)))
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeReplayedExpired(delivered) ==
    /\ phase = "Expired"
    /\ (IF ((notice = "Delivered") /\ delivered) THEN TRUE ELSE ((notice = "Failed") /\ ~(delivered)))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeReplayedWithdrawn(delivered) ==
    /\ phase = "Withdrawn"
    /\ (IF ((notice = "Delivered") /\ delivered) THEN TRUE ELSE ((notice = "Failed") /\ ~(delivered)))
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeReplayedExecuted(delivered) ==
    /\ phase = "Executed"
    /\ (IF ((notice = "Delivered") /\ delivered) THEN TRUE ELSE ((notice = "Failed") /\ ~(delivered)))
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeReplayedExecutionRefused(delivered) ==
    /\ phase = "ExecutionRefused"
    /\ (IF ((notice = "Delivered") /\ delivered) THEN TRUE ELSE ((notice = "Failed") /\ ~(delivered)))
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeRefusedRejected(delivered) ==
    /\ phase = "Rejected"
    /\ (IF (notice = "NotRequired") THEN TRUE ELSE (IF ((notice = "Delivered") /\ ~(delivered)) THEN TRUE ELSE ((notice = "Failed") /\ delivered)))
    /\ phase' = "Rejected"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeRefusedExpired(delivered) ==
    /\ phase = "Expired"
    /\ (IF (notice = "NotRequired") THEN TRUE ELSE (IF ((notice = "Delivered") /\ ~(delivered)) THEN TRUE ELSE ((notice = "Failed") /\ delivered)))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeRefusedWithdrawn(delivered) ==
    /\ phase = "Withdrawn"
    /\ (IF (notice = "NotRequired") THEN TRUE ELSE (IF ((notice = "Delivered") /\ ~(delivered)) THEN TRUE ELSE ((notice = "Failed") /\ delivered)))
    /\ phase' = "Withdrawn"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeRefusedExecuted(delivered) ==
    /\ phase = "Executed"
    /\ (IF (notice = "NotRequired") THEN TRUE ELSE (IF ((notice = "Delivered") /\ ~(delivered)) THEN TRUE ELSE ((notice = "Failed") /\ delivered)))
    /\ phase' = "Executed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeRefusedExecutionRefused(delivered) ==
    /\ phase = "ExecutionRefused"
    /\ (IF (notice = "NotRequired") THEN TRUE ELSE (IF ((notice = "Delivered") /\ ~(delivered)) THEN TRUE ELSE ((notice = "Failed") /\ delivered)))
    /\ phase' = "ExecutionRefused"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeRefusedNotEndedOpen(delivered) ==
    /\ phase = "Open"
    /\ phase' = "Open"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


SettleNoticeRefusedNotEndedApproved(delivered) ==
    /\ phase = "Approved"
    /\ phase' = "Approved"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f7d4c4b3385291f4


Next ==
    \/ (phase = "Unheld") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldHuman(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Unheld") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReview(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Unheld") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldRefusedWindow(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Unheld") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldRefusedSelfReview(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Open") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReplayedOpen(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Approved") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReplayedApproved(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Rejected") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReplayedRejected(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Expired") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReplayedExpired(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Withdrawn") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReplayedWithdrawn(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Executed") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReplayedExecuted(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "ExecutionRefused") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldReplayedExecutionRefused(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Open") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldConflictOpen(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Approved") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldConflictApproved(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Rejected") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldConflictRejected(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Expired") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldConflictExpired(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Withdrawn") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldConflictWithdrawn(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Executed") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldConflictExecuted(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "ExecutionRefused") /\ \E arg_decision_id \in StringValues : \E now_ms \in 0..2 : \E arg_deadline_at_ms \in 0..2 : \E arg_subject_digest \in DecisionDigestValues : \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E arg_requester \in DecisionPartyValues : \E arg_approver_id \in DecisionPartyValues : \E arg_reviewer \in OptionDecisionPartyValues : \E notice_routable \in BOOLEAN : HoldConflictExecutionRefused(arg_decision_id, now_ms, arg_deadline_at_ms, arg_subject_digest, arg_operation_id, arg_request_digest, arg_requester, arg_approver_id, arg_reviewer, notice_routable)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentAtDeadline(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedSubjectChanged(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedIncomplete(audience, digest, FALSE, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentToApprover(audience, digest, TRUE, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedUntimed(audience, digest, TRUE, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedWrongApproverAudience(audience, digest, TRUE, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentToReviewer(audience, digest, TRUE, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedWrongReviewerAudience(audience, digest, TRUE, presented_at_ms, now_ms)
    \/ (phase = "Approved") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedNotOpenApproved(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "Rejected") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedNotOpenRejected(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "Expired") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedNotOpenExpired(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "Withdrawn") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedNotOpenWithdrawn(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "Executed") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedNotOpenExecuted(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "ExecutionRefused") /\ \E audience \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E complete \in BOOLEAN : \E presented_at_ms \in 0..2 : \E now_ms \in 0..2 : PresentRefusedNotOpenExecutionRefused(audience, digest, complete, presented_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideAtDeadline(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedReviewStage(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedEvidenceRequired(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedSubjectChanged(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedNotEligible(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedSelfApproval(approve, arg_approver_id, decider, digest, arg_evidence_digest, TRUE, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedPresentationIncomplete(approve, arg_approver_id, decider, digest, arg_evidence_digest, TRUE, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedDecidedBeforePresentation(approve, arg_approver_id, decider, digest, arg_evidence_digest, TRUE, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideApproved(TRUE, arg_approver_id, decider, digest, arg_evidence_digest, TRUE, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRejected(FALSE, arg_approver_id, decider, digest, arg_evidence_digest, TRUE, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Approved") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedNotOpenApproved(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Rejected") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedNotOpenRejected(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Expired") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedNotOpenExpired(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Withdrawn") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedNotOpenWithdrawn(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Executed") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedNotOpenExecuted(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "ExecutionRefused") /\ \E approve \in BOOLEAN : \E arg_approver_id \in DecisionPartyValues : \E decider \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E eligible \in BOOLEAN : \E carried_presentation_at_ms \in OptionU64Values : \E observed_at_ms \in 0..2 : \E now_ms \in 0..2 : DecideRefusedNotOpenExecutionRefused(approve, arg_approver_id, decider, digest, arg_evidence_digest, eligible, carried_presentation_at_ms, observed_at_ms, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewAtDeadline(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedHumanStage(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedWrongReviewer(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedEvidenceRequired(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedSubjectChanged(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedPresentationIncomplete(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewAllowed(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewDenied(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewEscalated(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Approved") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedNotOpenApproved(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Rejected") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedNotOpenRejected(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Expired") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedNotOpenExpired(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Withdrawn") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedNotOpenWithdrawn(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Executed") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedNotOpenExecuted(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "ExecutionRefused") /\ \E verdict \in DurableDecisionReviewVerdictValues : \E arg_reviewer \in DecisionPartyValues : \E digest \in DecisionDigestValues : \E arg_evidence_digest \in OptionStringValues : \E now_ms \in 0..2 : ReviewRefusedNotOpenExecutionRefused(verdict, arg_reviewer, digest, arg_evidence_digest, now_ms)
    \/ (phase = "Open") /\ \E withdrawer \in DecisionPartyValues : \E by_host \in BOOLEAN : \E now_ms \in 0..2 : WithdrawAtDeadline(withdrawer, by_host, now_ms)
    \/ (phase = "Open") /\ \E withdrawer \in DecisionPartyValues : \E now_ms \in 0..2 : WithdrawByHost(withdrawer, TRUE, now_ms)
    \/ (phase = "Open") /\ \E withdrawer \in DecisionPartyValues : \E now_ms \in 0..2 : WithdrawByRequester(withdrawer, FALSE, now_ms)
    \/ (phase = "Open") /\ \E withdrawer \in DecisionPartyValues : \E now_ms \in 0..2 : WithdrawRefusedNotRequester(withdrawer, FALSE, now_ms)
    \/ (phase = "Approved") /\ \E withdrawer \in DecisionPartyValues : \E by_host \in BOOLEAN : \E now_ms \in 0..2 : WithdrawRefusedNotOpenApproved(withdrawer, by_host, now_ms)
    \/ (phase = "Rejected") /\ \E withdrawer \in DecisionPartyValues : \E by_host \in BOOLEAN : \E now_ms \in 0..2 : WithdrawRefusedNotOpenRejected(withdrawer, by_host, now_ms)
    \/ (phase = "Expired") /\ \E withdrawer \in DecisionPartyValues : \E by_host \in BOOLEAN : \E now_ms \in 0..2 : WithdrawRefusedNotOpenExpired(withdrawer, by_host, now_ms)
    \/ (phase = "Withdrawn") /\ \E withdrawer \in DecisionPartyValues : \E by_host \in BOOLEAN : \E now_ms \in 0..2 : WithdrawRefusedNotOpenWithdrawn(withdrawer, by_host, now_ms)
    \/ (phase = "Executed") /\ \E withdrawer \in DecisionPartyValues : \E by_host \in BOOLEAN : \E now_ms \in 0..2 : WithdrawRefusedNotOpenExecuted(withdrawer, by_host, now_ms)
    \/ (phase = "ExecutionRefused") /\ \E withdrawer \in DecisionPartyValues : \E by_host \in BOOLEAN : \E now_ms \in 0..2 : WithdrawRefusedNotOpenExecutionRefused(withdrawer, by_host, now_ms)
    \/ (phase = "Open") /\ \E now_ms \in 0..2 : ExpireOpenDue(now_ms)
    \/ (phase = "Open") /\ \E now_ms \in 0..2 : ExpireOpenNotDue(now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : ExpireApprovedDue(now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : ExpireApprovedSettlementOwed(now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : ExpireApprovedNotDue(now_ms)
    \/ (phase = "Rejected") /\ \E now_ms \in 0..2 : ExpireSettledRejected(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : ExpireSettledExpired(now_ms)
    \/ (phase = "Withdrawn") /\ \E now_ms \in 0..2 : ExpireSettledWithdrawn(now_ms)
    \/ (phase = "Executed") /\ \E now_ms \in 0..2 : ExpireSettledExecuted(now_ms)
    \/ (phase = "ExecutionRefused") /\ \E now_ms \in 0..2 : ExpireSettledExecutionRefused(now_ms)
    \/ (phase = "Approved") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecution(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Approved") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionAtDeadline(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Approved") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedSettlementOwed(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Approved") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedBinding(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Open") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedNotApprovedOpen(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Rejected") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedNotApprovedRejected(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Expired") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedNotApprovedExpired(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Withdrawn") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedNotApprovedWithdrawn(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Executed") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedNotApprovedExecuted(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "ExecutionRefused") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : AuthorizeExecutionRefusedNotApprovedExecutionRefused(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : ReleaseExecution(now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : ReleaseExecutionAtDeadline(now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : ReleaseExecutionRefusedNotAuthorized(now_ms)
    \/ (phase = "Open") /\ \E now_ms \in 0..2 : ReleaseExecutionRefusedNotApprovedOpen(now_ms)
    \/ (phase = "Rejected") /\ \E now_ms \in 0..2 : ReleaseExecutionRefusedNotApprovedRejected(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : ReleaseExecutionRefusedNotApprovedExpired(now_ms)
    \/ (phase = "Withdrawn") /\ \E now_ms \in 0..2 : ReleaseExecutionRefusedNotApprovedWithdrawn(now_ms)
    \/ (phase = "Executed") /\ \E now_ms \in 0..2 : ReleaseExecutionRefusedNotApprovedExecuted(now_ms)
    \/ (phase = "ExecutionRefused") /\ \E now_ms \in 0..2 : ReleaseExecutionRefusedNotApprovedExecutionRefused(now_ms)
    \/ (phase = "Approved") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecuted(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Approved") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedRefusedNotAuthorized(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Approved") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedRefusedBinding(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Executed") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedReplayed(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Executed") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedReplayRefusedBinding(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Open") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedRefusedNotApprovedOpen(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Rejected") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedRefusedNotApprovedRejected(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Expired") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedRefusedNotApprovedExpired(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Withdrawn") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedRefusedNotApprovedWithdrawn(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "ExecutionRefused") /\ \E arg_operation_id \in StringValues : \E arg_request_digest \in DecisionDigestValues : \E now_ms \in 0..2 : RecordExecutedRefusedNotApprovedExecutionRefused(arg_operation_id, arg_request_digest, now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : CloseRefused(now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : CloseRefusedWithPermit(now_ms)
    \/ (phase = "Approved") /\ \E now_ms \in 0..2 : CloseRefusedAtDeadline(now_ms)
    \/ (phase = "Open") /\ \E now_ms \in 0..2 : CloseRefusedNotApprovedOpen(now_ms)
    \/ (phase = "Rejected") /\ \E now_ms \in 0..2 : CloseRefusedNotApprovedRejected(now_ms)
    \/ (phase = "Expired") /\ \E now_ms \in 0..2 : CloseRefusedNotApprovedExpired(now_ms)
    \/ (phase = "Withdrawn") /\ \E now_ms \in 0..2 : CloseRefusedNotApprovedWithdrawn(now_ms)
    \/ (phase = "Executed") /\ \E now_ms \in 0..2 : CloseRefusedNotApprovedExecuted(now_ms)
    \/ (phase = "ExecutionRefused") /\ \E now_ms \in 0..2 : CloseRefusedNotApprovedExecutionRefused(now_ms)
    \/ SettleNoticeDeliveredRejected(TRUE)
    \/ SettleNoticeDeliveredExpired(TRUE)
    \/ SettleNoticeDeliveredWithdrawn(TRUE)
    \/ SettleNoticeDeliveredExecuted(TRUE)
    \/ SettleNoticeDeliveredExecutionRefused(TRUE)
    \/ SettleNoticeFailedRejected(FALSE)
    \/ SettleNoticeFailedExpired(FALSE)
    \/ SettleNoticeFailedWithdrawn(FALSE)
    \/ SettleNoticeFailedExecuted(FALSE)
    \/ SettleNoticeFailedExecutionRefused(FALSE)
    \/ (phase = "Rejected") /\ \E delivered \in BOOLEAN : SettleNoticeReplayedRejected(delivered)
    \/ (phase = "Expired") /\ \E delivered \in BOOLEAN : SettleNoticeReplayedExpired(delivered)
    \/ (phase = "Withdrawn") /\ \E delivered \in BOOLEAN : SettleNoticeReplayedWithdrawn(delivered)
    \/ (phase = "Executed") /\ \E delivered \in BOOLEAN : SettleNoticeReplayedExecuted(delivered)
    \/ (phase = "ExecutionRefused") /\ \E delivered \in BOOLEAN : SettleNoticeReplayedExecutionRefused(delivered)
    \/ (phase = "Rejected") /\ \E delivered \in BOOLEAN : SettleNoticeRefusedRejected(delivered)
    \/ (phase = "Expired") /\ \E delivered \in BOOLEAN : SettleNoticeRefusedExpired(delivered)
    \/ (phase = "Withdrawn") /\ \E delivered \in BOOLEAN : SettleNoticeRefusedWithdrawn(delivered)
    \/ (phase = "Executed") /\ \E delivered \in BOOLEAN : SettleNoticeRefusedExecuted(delivered)
    \/ (phase = "ExecutionRefused") /\ \E delivered \in BOOLEAN : SettleNoticeRefusedExecutionRefused(delivered)
    \/ (phase = "Open") /\ \E delivered \in BOOLEAN : SettleNoticeRefusedNotEndedOpen(delivered)
    \/ (phase = "Approved") /\ \E delivered \in BOOLEAN : SettleNoticeRefusedNotEndedApproved(delivered)
    \/ TerminalStutter

unheld_is_empty == (IF (phase # "Unheld") THEN TRUE ELSE ((decision_id = "") /\ (operation_id = "") /\ (deadline_at_ms = 0) /\ (reviewer = None) /\ (stage = "Human") /\ (first_presented_to_approver_at_ms = None) /\ ~(presented_to_reviewer) /\ (evidence_digest = None) /\ (decided_at_ms = None) /\ (ended_at_ms = None) /\ ~(permit_issued) /\ (executed_request_digest = None) /\ (notice = "NotRequired")))
held_records_are_named == (IF (phase = "Unheld") THEN TRUE ELSE ((decision_id # "") /\ (operation_id # "") /\ (deadline_at_ms > 0)))
review_stage_names_another_reviewer == (IF (stage # "Review") THEN TRUE ELSE ((reviewer # None) /\ ((IF "value" \in DOMAIN reviewer THEN reviewer["value"] ELSE None) # requester)))
reviewer_presentation_needs_a_reviewer == (IF ~(presented_to_reviewer) THEN TRUE ELSE (reviewer # None))
first_presentation_is_a_reading == (first_presented_to_approver_at_ms # Some(0))
open_has_no_outcome == (IF (phase # "Open") THEN TRUE ELSE ((evidence_digest = None) /\ (decided_at_ms = None) /\ (ended_at_ms = None) /\ ~(permit_issued) /\ (executed_request_digest = None) /\ (IF (notice = "NotRequired") THEN TRUE ELSE (notice = "NotDue"))))
approved_has_not_ended == (IF (phase # "Approved") THEN TRUE ELSE ((ended_at_ms = None) /\ (executed_request_digest = None) /\ (IF (notice = "NotRequired") THEN TRUE ELSE (notice = "NotDue"))))
evidence_and_decision_time_agree == ((evidence_digest = None) = (decided_at_ms = None))
decided_decisions_are_attested == (IF ((phase # "Approved") /\ (phase # "Rejected") /\ (phase # "Executed") /\ (phase # "ExecutionRefused")) THEN TRUE ELSE ((evidence_digest # None) /\ (decided_at_ms # None) /\ ((IF "value" \in DOMAIN decided_at_ms THEN decided_at_ms["value"] ELSE None) < deadline_at_ms)))
permit_is_outstanding_only_while_approved == (IF (phase = "Approved") THEN TRUE ELSE ~(permit_issued))
executed_names_its_held_request == (IF (phase # "Executed") THEN TRUE ELSE ((executed_request_digest # None) /\ ((IF "value" \in DOMAIN executed_request_digest THEN executed_request_digest["value"] ELSE None) = request_digest)))
only_executed_records_an_execution == (IF (phase = "Executed") THEN TRUE ELSE (executed_request_digest = None))
ended_decisions_record_their_end == (IF ((phase # "Rejected") /\ (phase # "Expired") /\ (phase # "Withdrawn") /\ (phase # "Executed") /\ (phase # "ExecutionRefused")) THEN TRUE ELSE (ended_at_ms # None))
expiry_never_precedes_its_deadline == (IF (phase # "Expired") THEN TRUE ELSE ((ended_at_ms # None) /\ ((IF "value" \in DOMAIN ended_at_ms THEN ended_at_ms["value"] ELSE None) >= deadline_at_ms) /\ (IF (decided_at_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN decided_at_ms THEN decided_at_ms["value"] ELSE None) < deadline_at_ms))))
withdrawal_precedes_the_deadline == (IF (phase # "Withdrawn") THEN TRUE ELSE ((ended_at_ms # None) /\ ((IF "value" \in DOMAIN ended_at_ms THEN ended_at_ms["value"] ELSE None) < deadline_at_ms) /\ (evidence_digest = None)))
ended_decisions_owe_their_notice == (IF ((phase # "Rejected") /\ (phase # "Expired") /\ (phase # "Withdrawn") /\ (phase # "Executed") /\ (phase # "ExecutionRefused")) THEN TRUE ELSE (notice # "NotDue"))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []unheld_is_empty
THEOREM Spec => []held_records_are_named
THEOREM Spec => []review_stage_names_another_reviewer
THEOREM Spec => []reviewer_presentation_needs_a_reviewer
THEOREM Spec => []first_presentation_is_a_reading
THEOREM Spec => []open_has_no_outcome
THEOREM Spec => []approved_has_not_ended
THEOREM Spec => []evidence_and_decision_time_agree
THEOREM Spec => []decided_decisions_are_attested
THEOREM Spec => []permit_is_outstanding_only_while_approved
THEOREM Spec => []executed_names_its_held_request
THEOREM Spec => []only_executed_records_an_execution
THEOREM Spec => []ended_decisions_record_their_end
THEOREM Spec => []expiry_never_precedes_its_deadline
THEOREM Spec => []withdrawal_precedes_the_deadline
THEOREM Spec => []ended_decisions_owe_their_notice

=============================================================================
