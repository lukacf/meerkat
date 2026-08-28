---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for TemporaryCouncilLifecycleMachine.

CONSTANTS BooleanValues, NatValues, StringValues, TemporaryCouncilAdvanceRejectionValues, TemporaryCouncilClaimDenialValues, TemporaryCouncilCleanupRejectionValues, TemporaryCouncilExitClassValues, TemporaryCouncilOpenRejectionValues, TemporaryCouncilSealRejectionValues

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

VARIABLES phase, model_step_count, revision, request_fingerprint, exit_class, cleanup_attempts, claim_id, claim_epoch

vars == << phase, model_step_count, revision, request_fingerprint, exit_class, cleanup_attempts, claim_id, claim_epoch >>

Init ==
    /\ phase = "Empty"
    /\ model_step_count = 0
    /\ revision = 0
    /\ request_fingerprint = ""
    /\ exit_class = "Unsealed"
    /\ cleanup_attempts = 0
    /\ claim_id = ""
    /\ claim_epoch = 0

TerminalStutter ==
    /\ phase = "Settled"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_49c3a15d7fce41f1 == UNCHANGED << request_fingerprint, cleanup_attempts, claim_id, claim_epoch >>
UnchangedFrame_4b4a995e293490fd == UNCHANGED << revision, request_fingerprint, exit_class, cleanup_attempts, claim_id, claim_epoch >>
UnchangedFrame_5be773814710de51 == UNCHANGED << revision, request_fingerprint, exit_class, cleanup_attempts >>
UnchangedFrame_809c18d299720f12 == UNCHANGED << request_fingerprint, exit_class, cleanup_attempts, claim_id, claim_epoch >>
UnchangedFrame_b8bd5ac6cb26c0bd == UNCHANGED << request_fingerprint, exit_class, claim_id, claim_epoch >>
UnchangedFrame_cfa832656d08efbc == UNCHANGED << exit_class, cleanup_attempts, claim_id, claim_epoch >>

OpenEmpty(arg_request_fingerprint) ==
    /\ phase = "Empty"
    /\ (arg_request_fingerprint # "")
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ request_fingerprint' = arg_request_fingerprint
    /\ UnchangedFrame_cfa832656d08efbc


OpenEmptyMalformed(arg_request_fingerprint) ==
    /\ phase = "Empty"
    /\ (arg_request_fingerprint = "")
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenReplayPreparing(arg_request_fingerprint) ==
    /\ phase = "Preparing"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenReplayRunning(arg_request_fingerprint) ==
    /\ phase = "Running"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenReplayMerging(arg_request_fingerprint) ==
    /\ phase = "Merging"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenReplayConcluded(arg_request_fingerprint) ==
    /\ phase = "Concluded"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenReplayCleanupDebt(arg_request_fingerprint) ==
    /\ phase = "CleanupDebt"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenReplaySettled(arg_request_fingerprint) ==
    /\ phase = "Settled"
    /\ (arg_request_fingerprint = request_fingerprint)
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenConflictPreparing(arg_request_fingerprint) ==
    /\ phase = "Preparing"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenConflictRunning(arg_request_fingerprint) ==
    /\ phase = "Running"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenConflictMerging(arg_request_fingerprint) ==
    /\ phase = "Merging"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenConflictConcluded(arg_request_fingerprint) ==
    /\ phase = "Concluded"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenConflictCleanupDebt(arg_request_fingerprint) ==
    /\ phase = "CleanupDebt"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


OpenConflictSettled(arg_request_fingerprint) ==
    /\ phase = "Settled"
    /\ (arg_request_fingerprint # request_fingerprint)
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimNotOpened(arg_claim_id, lease_expired) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimSettled(arg_claim_id, lease_expired) ==
    /\ phase = "Settled"
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimMalformedPreparing(arg_claim_id, lease_expired) ==
    /\ phase = "Preparing"
    /\ (arg_claim_id = "")
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimMalformedRunning(arg_claim_id, lease_expired) ==
    /\ phase = "Running"
    /\ (arg_claim_id = "")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimMalformedMerging(arg_claim_id, lease_expired) ==
    /\ phase = "Merging"
    /\ (arg_claim_id = "")
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimMalformedConcluded(arg_claim_id, lease_expired) ==
    /\ phase = "Concluded"
    /\ (arg_claim_id = "")
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimMalformedCleanupDebt(arg_claim_id, lease_expired) ==
    /\ phase = "CleanupDebt"
    /\ (arg_claim_id = "")
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimGrantPreparing(arg_claim_id, lease_expired) ==
    /\ phase = "Preparing"
    /\ (arg_claim_id # "")
    /\ (claim_id = "")
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimGrantRunning(arg_claim_id, lease_expired) ==
    /\ phase = "Running"
    /\ (arg_claim_id # "")
    /\ (claim_id = "")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimGrantMerging(arg_claim_id, lease_expired) ==
    /\ phase = "Merging"
    /\ (arg_claim_id # "")
    /\ (claim_id = "")
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimGrantConcluded(arg_claim_id, lease_expired) ==
    /\ phase = "Concluded"
    /\ (arg_claim_id # "")
    /\ (claim_id = "")
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimGrantCleanupDebt(arg_claim_id, lease_expired) ==
    /\ phase = "CleanupDebt"
    /\ (arg_claim_id # "")
    /\ (claim_id = "")
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimRenewPreparing(arg_claim_id, lease_expired) ==
    /\ phase = "Preparing"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id = claim_id))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimRenewRunning(arg_claim_id, lease_expired) ==
    /\ phase = "Running"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id = claim_id))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimRenewMerging(arg_claim_id, lease_expired) ==
    /\ phase = "Merging"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id = claim_id))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimRenewConcluded(arg_claim_id, lease_expired) ==
    /\ phase = "Concluded"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id = claim_id))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimRenewCleanupDebt(arg_claim_id, lease_expired) ==
    /\ phase = "CleanupDebt"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id = claim_id))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimTakeoverPreparing(arg_claim_id, lease_expired) ==
    /\ phase = "Preparing"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ lease_expired
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimTakeoverRunning(arg_claim_id, lease_expired) ==
    /\ phase = "Running"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ lease_expired
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimTakeoverMerging(arg_claim_id, lease_expired) ==
    /\ phase = "Merging"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ lease_expired
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimTakeoverConcluded(arg_claim_id, lease_expired) ==
    /\ phase = "Concluded"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ lease_expired
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimTakeoverCleanupDebt(arg_claim_id, lease_expired) ==
    /\ phase = "CleanupDebt"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ lease_expired
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ claim_id' = arg_claim_id
    /\ claim_epoch' = (claim_epoch) + 1
    /\ UnchangedFrame_5be773814710de51


ClaimBusyPreparing(arg_claim_id, lease_expired) ==
    /\ phase = "Preparing"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ (lease_expired = FALSE)
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimBusyRunning(arg_claim_id, lease_expired) ==
    /\ phase = "Running"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ (lease_expired = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimBusyMerging(arg_claim_id, lease_expired) ==
    /\ phase = "Merging"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ (lease_expired = FALSE)
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimBusyConcluded(arg_claim_id, lease_expired) ==
    /\ phase = "Concluded"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ (lease_expired = FALSE)
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClaimBusyCleanupDebt(arg_claim_id, lease_expired) ==
    /\ phase = "CleanupDebt"
    /\ (arg_claim_id # "")
    /\ ((claim_id # "") /\ (arg_claim_id # claim_id))
    /\ (lease_expired = FALSE)
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_809c18d299720f12


StartDiscussionReplay(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionNotOpened(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionAlreadyAdvancedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionAlreadyAdvancedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionAlreadyAdvancedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionAlreadyAdvancedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergePreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_809c18d299720f12


StartMergeRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ UnchangedFrame_809c18d299720f12


StartMergeReplay(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeNotOpened(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeAlreadyAdvancedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeAlreadyAdvancedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeAlreadyAdvancedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ exit_class' = "Executed"
    /\ UnchangedFrame_49c3a15d7fce41f1


SealResultReplayConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (exit_class = "Executed")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultReplayCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (exit_class = "Executed")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultReplaySettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (exit_class = "Executed")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultConflictConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (exit_class # "Executed")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultConflictCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (exit_class # "Executed")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultConflictSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (exit_class # "Executed")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultNotOpened(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultNotMergingPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultNotMergingRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ exit_class' = "CoordinatorInterrupted"
    /\ UnchangedFrame_49c3a15d7fce41f1


SealInterruptedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ exit_class' = "CoordinatorInterrupted"
    /\ UnchangedFrame_49c3a15d7fce41f1


SealInterruptedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ exit_class' = "CoordinatorInterrupted"
    /\ UnchangedFrame_49c3a15d7fce41f1


SealInterruptedReplayConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (exit_class = "CoordinatorInterrupted")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedReplayCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (exit_class = "CoordinatorInterrupted")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedReplaySettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (exit_class = "CoordinatorInterrupted")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedConflictConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (exit_class # "CoordinatorInterrupted")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedConflictCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (exit_class # "CoordinatorInterrupted")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedConflictSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (exit_class # "CoordinatorInterrupted")
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedNotOpened(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ cleanup_attempts' = (cleanup_attempts) + 1
    /\ UnchangedFrame_b8bd5ac6cb26c0bd


RecordCleanupSettledAfterDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ cleanup_attempts' = (cleanup_attempts) + 1
    /\ UnchangedFrame_b8bd5ac6cb26c0bd


RecordCleanupSettledReplay(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledNotSealedEmpty(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledNotSealedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledNotSealedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledNotSealedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ cleanup_attempts' = (cleanup_attempts) + 1
    /\ UnchangedFrame_b8bd5ac6cb26c0bd


RecordCleanupDebtRetry(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ cleanup_attempts' = (cleanup_attempts) + 1
    /\ UnchangedFrame_b8bd5ac6cb26c0bd


RecordCleanupDebtAlreadySettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtNotSealedEmpty(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtNotSealedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtNotSealedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtNotSealedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ ((arg_claim_id = claim_id) /\ (arg_claim_epoch = claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionFencedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionFencedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionFencedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionFencedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionFencedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartDiscussionFencedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeFencedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeFencedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeFencedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeFencedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeFencedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


StartMergeFencedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultFencedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultFencedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultFencedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultFencedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultFencedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealResultFencedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedResultFencedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedResultFencedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedResultFencedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedResultFencedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedResultFencedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


SealInterruptedResultFencedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledFencedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledFencedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledFencedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledFencedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledFencedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupSettledFencedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtFencedPreparing(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Preparing"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtFencedRunning(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Running"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtFencedMerging(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Merging"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtFencedConcluded(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Concluded"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtFencedCleanupDebt(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "CleanupDebt"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


RecordCleanupDebtFencedSettled(arg_claim_id, arg_claim_epoch) ==
    /\ phase = "Settled"
    /\ (IF (arg_claim_id # claim_id) THEN TRUE ELSE (arg_claim_epoch # claim_epoch))
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClassifyRecoveryEmpty ==
    /\ phase = "Empty"
    /\ phase' = "Empty"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClassifyRecoveryPreparing ==
    /\ phase = "Preparing"
    /\ phase' = "Preparing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClassifyRecoveryRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClassifyRecoveryMerging ==
    /\ phase = "Merging"
    /\ phase' = "Merging"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClassifyRecoveryConcluded ==
    /\ phase = "Concluded"
    /\ phase' = "Concluded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClassifyRecoveryCleanupDebt ==
    /\ phase = "CleanupDebt"
    /\ phase' = "CleanupDebt"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


ClassifyRecoverySettled ==
    /\ phase = "Settled"
    /\ phase' = "Settled"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_4b4a995e293490fd


Next ==
    \/ \E arg_request_fingerprint \in StringValues : OpenEmpty(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenEmptyMalformed(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenReplayPreparing(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenReplayRunning(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenReplayMerging(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenReplayConcluded(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenReplayCleanupDebt(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenReplaySettled(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenConflictPreparing(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenConflictRunning(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenConflictMerging(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenConflictConcluded(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenConflictCleanupDebt(arg_request_fingerprint)
    \/ \E arg_request_fingerprint \in StringValues : OpenConflictSettled(arg_request_fingerprint)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimNotOpened(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimSettled(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimMalformedPreparing(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimMalformedRunning(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimMalformedMerging(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimMalformedConcluded(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimMalformedCleanupDebt(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimGrantPreparing(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimGrantRunning(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimGrantMerging(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimGrantConcluded(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimGrantCleanupDebt(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimRenewPreparing(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimRenewRunning(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimRenewMerging(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimRenewConcluded(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : \E lease_expired \in BOOLEAN : ClaimRenewCleanupDebt(arg_claim_id, lease_expired)
    \/ \E arg_claim_id \in StringValues : ClaimTakeoverPreparing(arg_claim_id, TRUE)
    \/ \E arg_claim_id \in StringValues : ClaimTakeoverRunning(arg_claim_id, TRUE)
    \/ \E arg_claim_id \in StringValues : ClaimTakeoverMerging(arg_claim_id, TRUE)
    \/ \E arg_claim_id \in StringValues : ClaimTakeoverConcluded(arg_claim_id, TRUE)
    \/ \E arg_claim_id \in StringValues : ClaimTakeoverCleanupDebt(arg_claim_id, TRUE)
    \/ \E arg_claim_id \in StringValues : ClaimBusyPreparing(arg_claim_id, FALSE)
    \/ \E arg_claim_id \in StringValues : ClaimBusyRunning(arg_claim_id, FALSE)
    \/ \E arg_claim_id \in StringValues : ClaimBusyMerging(arg_claim_id, FALSE)
    \/ \E arg_claim_id \in StringValues : ClaimBusyConcluded(arg_claim_id, FALSE)
    \/ \E arg_claim_id \in StringValues : ClaimBusyCleanupDebt(arg_claim_id, FALSE)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionReplay(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionNotOpened(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionAlreadyAdvancedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionAlreadyAdvancedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionAlreadyAdvancedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionAlreadyAdvancedSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergePreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeReplay(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeNotOpened(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeAlreadyAdvancedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeAlreadyAdvancedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeAlreadyAdvancedSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultReplayConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultReplayCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultReplaySettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultConflictConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultConflictCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultConflictSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultNotOpened(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultNotMergingPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultNotMergingRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedReplayConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedReplayCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedReplaySettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedConflictConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedConflictCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedConflictSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedNotOpened(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledAfterDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledReplay(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledNotSealedEmpty(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledNotSealedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledNotSealedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledNotSealedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtRetry(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtAlreadySettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtNotSealedEmpty(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtNotSealedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtNotSealedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtNotSealedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionFencedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionFencedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionFencedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionFencedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionFencedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartDiscussionFencedSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeFencedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeFencedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeFencedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeFencedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeFencedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : StartMergeFencedSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultFencedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultFencedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultFencedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultFencedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultFencedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealResultFencedSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedResultFencedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedResultFencedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedResultFencedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedResultFencedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedResultFencedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : SealInterruptedResultFencedSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledFencedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledFencedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledFencedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledFencedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledFencedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupSettledFencedSettled(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtFencedPreparing(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtFencedRunning(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtFencedMerging(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtFencedConcluded(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtFencedCleanupDebt(arg_claim_id, arg_claim_epoch)
    \/ \E arg_claim_id \in StringValues : \E arg_claim_epoch \in 0..2 : RecordCleanupDebtFencedSettled(arg_claim_id, arg_claim_epoch)
    \/ ClassifyRecoveryEmpty
    \/ ClassifyRecoveryPreparing
    \/ ClassifyRecoveryRunning
    \/ ClassifyRecoveryMerging
    \/ ClassifyRecoveryConcluded
    \/ ClassifyRecoveryCleanupDebt
    \/ ClassifyRecoverySettled
    \/ TerminalStutter

empty_record_has_no_council_facts == (IF (phase # "Empty") THEN TRUE ELSE ((request_fingerprint = "") /\ (exit_class = "Unsealed") /\ (cleanup_attempts = 0) /\ (revision = 0)))
opened_record_is_fingerprint_bound == (IF (phase = "Empty") THEN TRUE ELSE (request_fingerprint # ""))
sealed_phase_has_exit_class == (IF ((phase # "Concluded") /\ (phase # "CleanupDebt") /\ (phase # "Settled")) THEN TRUE ELSE (exit_class # "Unsealed"))
unsealed_phase_has_no_exit_class == (IF ((phase # "Empty") /\ (phase # "Preparing") /\ (phase # "Running") /\ (phase # "Merging")) THEN TRUE ELSE (exit_class = "Unsealed"))
cleanup_attempts_require_a_sealed_result == (IF (cleanup_attempts = 0) THEN TRUE ELSE (IF (phase = "CleanupDebt") THEN TRUE ELSE (phase = "Settled")))
debt_and_settlement_have_attempts == (IF ((phase # "CleanupDebt") /\ (phase # "Settled")) THEN TRUE ELSE (cleanup_attempts > 0))
claim_identity_and_epoch_agree == (IF ((claim_id = "") /\ (claim_epoch = 0)) THEN TRUE ELSE ((claim_id # "") /\ (claim_epoch > 0)))
only_a_bound_record_is_claimable == (IF (phase # "Empty") THEN TRUE ELSE (claim_id = ""))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []empty_record_has_no_council_facts
THEOREM Spec => []opened_record_is_fingerprint_bound
THEOREM Spec => []sealed_phase_has_exit_class
THEOREM Spec => []unsealed_phase_has_no_exit_class
THEOREM Spec => []cleanup_attempts_require_a_sealed_result
THEOREM Spec => []debt_and_settlement_have_attempts
THEOREM Spec => []claim_identity_and_epoch_agree
THEOREM Spec => []only_a_bound_record_is_claimable

=============================================================================
