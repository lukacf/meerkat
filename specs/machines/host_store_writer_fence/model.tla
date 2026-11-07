---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for HostStoreWriterFenceMachine.

CONSTANTS BooleanValues, NatValues, ScopePoisonCauseValues, SetOfStringValues, StringValues, WriterFenceAcquireOutcomeValues, WriterFenceAcquireStepValues, WriterFenceListingOutcomeValues, WriterFenceLostCauseValues, WriterFenceRefusalValues, WriterFenceRenewOutcomeValues, WriterFenceStopCauseValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringScopePoisonCauseValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ScopePoisonCauseValues }
MapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in NatValues }
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
MapStringSetStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in SetOfStringValues }

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

VARIABLES phase, model_step_count, token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes

vars == << phase, model_step_count, token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>

Init ==
    /\ phase = "Unopened"
    /\ model_step_count = 0
    /\ token = 0
    /\ granted = None
    /\ own_grant = None
    /\ step = "Exclusive"
    /\ timed_out = 0
    /\ waited_holder = None
    /\ stop_cause = "RenewFailed"
    /\ lost_cause = "FencedOutRenew"
    /\ writer_stopped = FALSE
    /\ open_listing_failed = FALSE
    /\ quarantined_keys = {}
    /\ quarantine_versions = [x \in {} |-> None]
    /\ quarantine_commit_ids = [x \in {} |-> None]
    /\ poisoned_scopes = [x \in {} |-> None]

TerminalStutter ==
    /\ phase = "Lost"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_00c10643c84d7389 == UNCHANGED << token, granted, own_grant, step, waited_holder, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_137ac3f0a19c832c == UNCHANGED << token, granted, own_grant, step, timed_out, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_24f82e11e61d4af2 == UNCHANGED << token, step, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_31772fce1a106a39 == UNCHANGED << token, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_4696d43a660b8338 == UNCHANGED << token, own_grant, step, timed_out, waited_holder, stop_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_5da5664c4434ebaa == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids >>
UnchangedFrame_6e2162eac3ba34b0 == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_7ce65679b203ea7c == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, lost_cause, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_848ddefb03e90937 == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, poisoned_scopes >>
UnchangedFrame_8df0ee4ccb177ce9 == UNCHANGED << granted, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_9ce9f28fd4fa9a95 == UNCHANGED << token, granted, own_grant, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_9db99e3860a37faf == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_a66417a329cf0764 == UNCHANGED << granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_d76d071c1c5bf655 == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, stop_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_f3d8f0e3133862c7 == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>
UnchangedFrame_f9f297b3af911b82 == UNCHANGED << token, own_grant, step, timed_out, waited_holder, lost_cause, writer_stopped, open_listing_failed, quarantined_keys, quarantine_versions, quarantine_commit_ids, poisoned_scopes >>

Open(arg_token) ==
    /\ phase = "Unopened"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ token' = arg_token
    /\ UnchangedFrame_a66417a329cf0764


OpenAgainLive(arg_token) ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenAgainStopped(arg_token) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenAgainAcquiring(arg_token) ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenAgainProving(arg_token) ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenAgainResuming(arg_token) ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenAgainWaitingOut(arg_token) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenLost(arg_token) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


WindowClosedLive ==
    /\ phase = "Live"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "WindowClosed"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_7ce65679b203ea7c


WindowClosedNotLiveStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


WindowClosedNotLiveAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


WindowClosedNotLiveProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


WindowClosedNotLiveResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


WindowClosedNotLiveWaitingOut ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


WindowClosedUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


WindowClosedLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewedInWindow(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "Renewed")
    /\ in_window
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewedAfterTheWindow(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "Renewed")
    /\ ~(in_window)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "WindowClosedBeforeRenew"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_7ce65679b203ea7c


RenewStaleLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "FencedOut")
    /\ ((current # renewed) /\ (current = token))
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewFencedOutLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "FencedOut")
    /\ (IF (current = renewed) THEN TRUE ELSE (current # token))
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "FencedOutRenew"
    /\ UnchangedFrame_d76d071c1c5bf655


RenewFailedLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "RenewFailed"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_7ce65679b203ea7c


RenewTimedOutLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "TimedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "RenewTimedOut"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_7ce65679b203ea7c


RenewAnsweredStopped(outcome, renewed, current, in_window) ==
    /\ phase = "Stopped"
    /\ (outcome # "FencedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewStaleStopped(outcome, renewed, current, in_window) ==
    /\ phase = "Stopped"
    /\ (outcome = "FencedOut")
    /\ ((current # renewed) /\ (current = token))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewFencedOutStopped(outcome, renewed, current, in_window) ==
    /\ phase = "Stopped"
    /\ (outcome = "FencedOut")
    /\ (IF (current = renewed) THEN TRUE ELSE (current # token))
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "FencedOutRenew"
    /\ UnchangedFrame_d76d071c1c5bf655


ResumeRenewed(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "Renewed")
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ writer_stopped' = FALSE
    /\ UnchangedFrame_31772fce1a106a39


ResumeFencedOut(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "FencedOut")
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ lost_cause' = "FencedOutAfterReacquire"
    /\ UnchangedFrame_4696d43a660b8338


ResumeRenewFailed(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ stop_cause' = "ResumeRenewFailed"
    /\ UnchangedFrame_f9f297b3af911b82


ResumeRenewTimedOut(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "TimedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ stop_cause' = "ResumeRenewTimedOut"
    /\ UnchangedFrame_f9f297b3af911b82


RenewAnsweredNoRenewUnopened(outcome, renewed, current, in_window) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewAnsweredNoRenewAcquiring(outcome, renewed, current, in_window) ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewAnsweredNoRenewProving(outcome, renewed, current, in_window) ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewAnsweredNoRenewWaitingOut(outcome, renewed, current, in_window) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


RenewAnsweredLost(outcome, renewed, current, in_window) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquire ==
    /\ phase = "Stopped"
    /\ ~(open_listing_failed)
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ step' = "Exclusive"
    /\ UnchangedFrame_9ce9f28fd4fa9a95


BeginReacquireAfterFailedOpen ==
    /\ phase = "Stopped"
    /\ open_listing_failed
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquireLive ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquireInFlightAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquireInFlightProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquireInFlightResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquireWaitingOut ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquireUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


BeginReacquireLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireGranted(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Granted")
    /\ (arg_token > token)
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = Some(arg_token)
    /\ own_grant' = Some(arg_token)
    /\ timed_out' = 0
    /\ waited_holder' = None
    /\ UnchangedFrame_24f82e11e61d4af2


AcquireGrantedOutOfOrder(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Granted")
    /\ (arg_token <= token)
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "TokenOrder"
    /\ UnchangedFrame_d76d071c1c5bf655


AcquireHeldByOwnLease(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "Exclusive")
    /\ (IF (arg_token = token) THEN TRUE ELSE (own_grant = Some(arg_token)))
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ step' = "TakeoverOwn"
    /\ UnchangedFrame_9ce9f28fd4fa9a95


AcquireHeldByAnother(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "Exclusive")
    /\ ((arg_token # token) /\ (own_grant # Some(arg_token)))
    /\ (timed_out = 0)
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "HeldByAnother"
    /\ UnchangedFrame_d76d071c1c5bf655


AcquireHeldByRenewedHolder(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "Exclusive")
    /\ ((arg_token # token) /\ (own_grant # Some(arg_token)))
    /\ (timed_out > 0)
    /\ (waited_holder = Some(arg_token))
    /\ lease_moved
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "HolderRenewed"
    /\ UnchangedFrame_d76d071c1c5bf655


AcquireHeldByUnrenewedHolder(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "Exclusive")
    /\ ((arg_token # token) /\ (own_grant # Some(arg_token)))
    /\ (timed_out > 0)
    /\ (waited_holder = Some(arg_token))
    /\ ~(lease_moved)
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireHeldByPossibleLateGrant(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "Exclusive")
    /\ ((arg_token # token) /\ (own_grant # Some(arg_token)))
    /\ (timed_out > 0)
    /\ (waited_holder # Some(arg_token))
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ waited_holder' = Some(arg_token)
    /\ UnchangedFrame_137ac3f0a19c832c


AcquireHeldAtTakeover(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "TakeoverOwn")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "ReacquireFailed"
    /\ UnchangedFrame_f3d8f0e3133862c7


AcquireFailed(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "ReacquireFailed"
    /\ UnchangedFrame_f3d8f0e3133862c7


AcquireTimedOut(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "TimedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ timed_out' = (timed_out) + 1
    /\ stop_cause' = "ReacquireTimedOut"
    /\ UnchangedFrame_00c10643c84d7389


AcquireAnsweredNotAcquiringUnopened(outcome, arg_token, lease_moved) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireAnsweredNotAcquiringLive(outcome, arg_token, lease_moved) ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireAnsweredNotAcquiringStopped(outcome, arg_token, lease_moved) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireAnsweredNotAcquiringProving(outcome, arg_token, lease_moved) ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireAnsweredNotAcquiringResuming(outcome, arg_token, lease_moved) ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireAnsweredNotAcquiringWaitingOut(outcome, arg_token, lease_moved) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


AcquireAnsweredLost(outcome, arg_token, lease_moved) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEnded ==
    /\ phase = "WaitingOut"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEndedNotWaitingOutUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEndedNotWaitingOutLive ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEndedNotWaitingOutStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEndedNotWaitingOutAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEndedNotWaitingOutProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEndedNotWaitingOutResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


LeaseEndedLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingClean(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted = Some(arg_token))
    /\ (outcome = "Clean")
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ token' = arg_token
    /\ own_grant' = None
    /\ UnchangedFrame_8df0ee4ccb177ce9


ListingForeign(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted = Some(arg_token))
    /\ (outcome = "ForeignBatch")
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ lost_cause' = "ForeignBatch"
    /\ UnchangedFrame_4696d43a660b8338


ListingFailed(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted = Some(arg_token))
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ stop_cause' = "ListingFailed"
    /\ UnchangedFrame_f9f297b3af911b82


ListingOtherGrant(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted # Some(arg_token))
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingCheckedNotProvingUnopened(outcome, arg_token) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingCheckedNotProvingLive(outcome, arg_token) ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingCheckedNotProvingStopped(outcome, arg_token) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingCheckedNotProvingAcquiring(outcome, arg_token) ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingCheckedNotProvingResuming(outcome, arg_token) ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingCheckedNotProvingWaitingOut(outcome, arg_token) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ListingCheckedLost(outcome, arg_token) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


Poison(cause) ==
    /\ phase = "Live" \/ phase = "Stopped" \/ phase = "Acquiring" \/ phase = "Proving" \/ phase = "Resuming" \/ phase = "WaitingOut"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ lost_cause' = cause
    /\ UnchangedFrame_4696d43a660b8338


PoisonUnopened(cause) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


PoisonLost(cause) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenListingFailedLive ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ open_listing_failed' = TRUE
    /\ UnchangedFrame_6e2162eac3ba34b0


OpenListingFailedStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ open_listing_failed' = TRUE
    /\ UnchangedFrame_6e2162eac3ba34b0


OpenListingFailedAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ open_listing_failed' = TRUE
    /\ UnchangedFrame_6e2162eac3ba34b0


OpenListingFailedProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ open_listing_failed' = TRUE
    /\ UnchangedFrame_6e2162eac3ba34b0


OpenListingFailedResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ open_listing_failed' = TRUE
    /\ UnchangedFrame_6e2162eac3ba34b0


OpenListingFailedWaitingOut ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ open_listing_failed' = TRUE
    /\ UnchangedFrame_6e2162eac3ba34b0


OpenListingFailedUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


OpenListingFailedLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedLive(key, name, version, commit_ids) ==
    /\ phase = "Live"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ ~((key \in DOMAIN poisoned_scopes))
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ quarantined_keys' = (quarantined_keys \cup {key})
    /\ quarantine_versions' = MapSet(quarantine_versions, name, version)
    /\ quarantine_commit_ids' = MapSet(quarantine_commit_ids, name, commit_ids)
    /\ UnchangedFrame_848ddefb03e90937


ScopeQuarantinedStopped(key, name, version, commit_ids) ==
    /\ phase = "Stopped"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ ~((key \in DOMAIN poisoned_scopes))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ quarantined_keys' = (quarantined_keys \cup {key})
    /\ quarantine_versions' = MapSet(quarantine_versions, name, version)
    /\ quarantine_commit_ids' = MapSet(quarantine_commit_ids, name, commit_ids)
    /\ UnchangedFrame_848ddefb03e90937


ScopeQuarantinedAcquiring(key, name, version, commit_ids) ==
    /\ phase = "Acquiring"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ ~((key \in DOMAIN poisoned_scopes))
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ quarantined_keys' = (quarantined_keys \cup {key})
    /\ quarantine_versions' = MapSet(quarantine_versions, name, version)
    /\ quarantine_commit_ids' = MapSet(quarantine_commit_ids, name, commit_ids)
    /\ UnchangedFrame_848ddefb03e90937


ScopeQuarantinedProving(key, name, version, commit_ids) ==
    /\ phase = "Proving"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ ~((key \in DOMAIN poisoned_scopes))
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ quarantined_keys' = (quarantined_keys \cup {key})
    /\ quarantine_versions' = MapSet(quarantine_versions, name, version)
    /\ quarantine_commit_ids' = MapSet(quarantine_commit_ids, name, commit_ids)
    /\ UnchangedFrame_848ddefb03e90937


ScopeQuarantinedResuming(key, name, version, commit_ids) ==
    /\ phase = "Resuming"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ ~((key \in DOMAIN poisoned_scopes))
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ quarantined_keys' = (quarantined_keys \cup {key})
    /\ quarantine_versions' = MapSet(quarantine_versions, name, version)
    /\ quarantine_commit_ids' = MapSet(quarantine_commit_ids, name, commit_ids)
    /\ UnchangedFrame_848ddefb03e90937


ScopeQuarantinedWaitingOut(key, name, version, commit_ids) ==
    /\ phase = "WaitingOut"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ ~((key \in DOMAIN poisoned_scopes))
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ quarantined_keys' = (quarantined_keys \cup {key})
    /\ quarantine_versions' = MapSet(quarantine_versions, name, version)
    /\ quarantine_commit_ids' = MapSet(quarantine_commit_ids, name, commit_ids)
    /\ UnchangedFrame_848ddefb03e90937


ScopeQuarantinedAgainLive(key, name, version, commit_ids) ==
    /\ phase = "Live"
    /\ (name \in DOMAIN quarantine_versions)
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedAgainStopped(key, name, version, commit_ids) ==
    /\ phase = "Stopped"
    /\ (name \in DOMAIN quarantine_versions)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedAgainAcquiring(key, name, version, commit_ids) ==
    /\ phase = "Acquiring"
    /\ (name \in DOMAIN quarantine_versions)
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedAgainProving(key, name, version, commit_ids) ==
    /\ phase = "Proving"
    /\ (name \in DOMAIN quarantine_versions)
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedAgainResuming(key, name, version, commit_ids) ==
    /\ phase = "Resuming"
    /\ (name \in DOMAIN quarantine_versions)
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedAgainWaitingOut(key, name, version, commit_ids) ==
    /\ phase = "WaitingOut"
    /\ (name \in DOMAIN quarantine_versions)
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedPoisonedLive(key, name, version, commit_ids) ==
    /\ phase = "Live"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ (key \in DOMAIN poisoned_scopes)
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedPoisonedStopped(key, name, version, commit_ids) ==
    /\ phase = "Stopped"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ (key \in DOMAIN poisoned_scopes)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedPoisonedAcquiring(key, name, version, commit_ids) ==
    /\ phase = "Acquiring"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ (key \in DOMAIN poisoned_scopes)
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedPoisonedProving(key, name, version, commit_ids) ==
    /\ phase = "Proving"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ (key \in DOMAIN poisoned_scopes)
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedPoisonedResuming(key, name, version, commit_ids) ==
    /\ phase = "Resuming"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ (key \in DOMAIN poisoned_scopes)
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedPoisonedWaitingOut(key, name, version, commit_ids) ==
    /\ phase = "WaitingOut"
    /\ ~((name \in DOMAIN quarantine_versions))
    /\ (key \in DOMAIN poisoned_scopes)
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedUnopened(key, name, version, commit_ids) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopeQuarantinedLost(key, name, version, commit_ids) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedLive(scope, cause) ==
    /\ phase = "Live"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ ~((scope \in quarantined_keys))
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ poisoned_scopes' = MapSet(poisoned_scopes, scope, cause)
    /\ UnchangedFrame_5da5664c4434ebaa


ScopePoisonedStopped(scope, cause) ==
    /\ phase = "Stopped"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ ~((scope \in quarantined_keys))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ poisoned_scopes' = MapSet(poisoned_scopes, scope, cause)
    /\ UnchangedFrame_5da5664c4434ebaa


ScopePoisonedAcquiring(scope, cause) ==
    /\ phase = "Acquiring"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ ~((scope \in quarantined_keys))
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ poisoned_scopes' = MapSet(poisoned_scopes, scope, cause)
    /\ UnchangedFrame_5da5664c4434ebaa


ScopePoisonedProving(scope, cause) ==
    /\ phase = "Proving"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ ~((scope \in quarantined_keys))
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ poisoned_scopes' = MapSet(poisoned_scopes, scope, cause)
    /\ UnchangedFrame_5da5664c4434ebaa


ScopePoisonedResuming(scope, cause) ==
    /\ phase = "Resuming"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ ~((scope \in quarantined_keys))
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ poisoned_scopes' = MapSet(poisoned_scopes, scope, cause)
    /\ UnchangedFrame_5da5664c4434ebaa


ScopePoisonedWaitingOut(scope, cause) ==
    /\ phase = "WaitingOut"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ ~((scope \in quarantined_keys))
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ poisoned_scopes' = MapSet(poisoned_scopes, scope, cause)
    /\ UnchangedFrame_5da5664c4434ebaa


ScopePoisonedAgainLive(scope, cause) ==
    /\ phase = "Live"
    /\ (scope \in DOMAIN poisoned_scopes)
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedAgainStopped(scope, cause) ==
    /\ phase = "Stopped"
    /\ (scope \in DOMAIN poisoned_scopes)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedAgainAcquiring(scope, cause) ==
    /\ phase = "Acquiring"
    /\ (scope \in DOMAIN poisoned_scopes)
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedAgainProving(scope, cause) ==
    /\ phase = "Proving"
    /\ (scope \in DOMAIN poisoned_scopes)
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedAgainResuming(scope, cause) ==
    /\ phase = "Resuming"
    /\ (scope \in DOMAIN poisoned_scopes)
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedAgainWaitingOut(scope, cause) ==
    /\ phase = "WaitingOut"
    /\ (scope \in DOMAIN poisoned_scopes)
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedQuarantinedLive(scope, cause) ==
    /\ phase = "Live"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ (scope \in quarantined_keys)
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedQuarantinedStopped(scope, cause) ==
    /\ phase = "Stopped"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ (scope \in quarantined_keys)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedQuarantinedAcquiring(scope, cause) ==
    /\ phase = "Acquiring"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ (scope \in quarantined_keys)
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedQuarantinedProving(scope, cause) ==
    /\ phase = "Proving"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ (scope \in quarantined_keys)
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedQuarantinedResuming(scope, cause) ==
    /\ phase = "Resuming"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ (scope \in quarantined_keys)
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedQuarantinedWaitingOut(scope, cause) ==
    /\ phase = "WaitingOut"
    /\ ~((scope \in DOMAIN poisoned_scopes))
    /\ (scope \in quarantined_keys)
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedUnopened(scope, cause) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


ScopePoisonedLost(scope, cause) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_9db99e3860a37faf


Next ==
    \/ (phase = "Unopened") /\ \E arg_token \in 0..2 : Open(arg_token)
    \/ (phase = "Live") /\ \E arg_token \in 0..2 : OpenAgainLive(arg_token)
    \/ (phase = "Stopped") /\ \E arg_token \in 0..2 : OpenAgainStopped(arg_token)
    \/ (phase = "Acquiring") /\ \E arg_token \in 0..2 : OpenAgainAcquiring(arg_token)
    \/ (phase = "Proving") /\ \E arg_token \in 0..2 : OpenAgainProving(arg_token)
    \/ (phase = "Resuming") /\ \E arg_token \in 0..2 : OpenAgainResuming(arg_token)
    \/ (phase = "WaitingOut") /\ \E arg_token \in 0..2 : OpenAgainWaitingOut(arg_token)
    \/ (phase = "Lost") /\ \E arg_token \in 0..2 : OpenLost(arg_token)
    \/ WindowClosedLive
    \/ WindowClosedNotLiveStopped
    \/ WindowClosedNotLiveAcquiring
    \/ WindowClosedNotLiveProving
    \/ WindowClosedNotLiveResuming
    \/ WindowClosedNotLiveWaitingOut
    \/ WindowClosedUnopened
    \/ WindowClosedLost
    \/ (phase = "Live") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : RenewedInWindow(outcome, renewed, current, TRUE)
    \/ (phase = "Live") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : RenewedAfterTheWindow(outcome, renewed, current, FALSE)
    \/ (phase = "Live") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewStaleLive(outcome, renewed, current, in_window)
    \/ (phase = "Live") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewFencedOutLive(outcome, renewed, current, in_window)
    \/ (phase = "Live") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewFailedLive(outcome, renewed, current, in_window)
    \/ (phase = "Live") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewTimedOutLive(outcome, renewed, current, in_window)
    \/ (phase = "Stopped") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewAnsweredStopped(outcome, renewed, current, in_window)
    \/ (phase = "Stopped") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewStaleStopped(outcome, renewed, current, in_window)
    \/ (phase = "Stopped") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewFencedOutStopped(outcome, renewed, current, in_window)
    \/ (phase = "Resuming") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : ResumeRenewed(outcome, renewed, current, in_window)
    \/ (phase = "Resuming") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : ResumeFencedOut(outcome, renewed, current, in_window)
    \/ (phase = "Resuming") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : ResumeRenewFailed(outcome, renewed, current, in_window)
    \/ (phase = "Resuming") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : ResumeRenewTimedOut(outcome, renewed, current, in_window)
    \/ (phase = "Unopened") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewAnsweredNoRenewUnopened(outcome, renewed, current, in_window)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewAnsweredNoRenewAcquiring(outcome, renewed, current, in_window)
    \/ (phase = "Proving") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewAnsweredNoRenewProving(outcome, renewed, current, in_window)
    \/ (phase = "WaitingOut") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewAnsweredNoRenewWaitingOut(outcome, renewed, current, in_window)
    \/ (phase = "Lost") /\ \E outcome \in WriterFenceRenewOutcomeValues : \E renewed \in 0..2 : \E current \in 0..2 : \E in_window \in BOOLEAN : RenewAnsweredLost(outcome, renewed, current, in_window)
    \/ BeginReacquire
    \/ BeginReacquireAfterFailedOpen
    \/ BeginReacquireLive
    \/ BeginReacquireInFlightAcquiring
    \/ BeginReacquireInFlightProving
    \/ BeginReacquireInFlightResuming
    \/ BeginReacquireWaitingOut
    \/ BeginReacquireUnopened
    \/ BeginReacquireLost
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireGranted(outcome, arg_token, lease_moved)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireGrantedOutOfOrder(outcome, arg_token, lease_moved)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireHeldByOwnLease(outcome, arg_token, lease_moved)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireHeldByAnother(outcome, arg_token, lease_moved)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : AcquireHeldByRenewedHolder(outcome, arg_token, TRUE)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : AcquireHeldByUnrenewedHolder(outcome, arg_token, FALSE)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireHeldByPossibleLateGrant(outcome, arg_token, lease_moved)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireHeldAtTakeover(outcome, arg_token, lease_moved)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireFailed(outcome, arg_token, lease_moved)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireTimedOut(outcome, arg_token, lease_moved)
    \/ (phase = "Unopened") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireAnsweredNotAcquiringUnopened(outcome, arg_token, lease_moved)
    \/ (phase = "Live") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireAnsweredNotAcquiringLive(outcome, arg_token, lease_moved)
    \/ (phase = "Stopped") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireAnsweredNotAcquiringStopped(outcome, arg_token, lease_moved)
    \/ (phase = "Proving") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireAnsweredNotAcquiringProving(outcome, arg_token, lease_moved)
    \/ (phase = "Resuming") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireAnsweredNotAcquiringResuming(outcome, arg_token, lease_moved)
    \/ (phase = "WaitingOut") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireAnsweredNotAcquiringWaitingOut(outcome, arg_token, lease_moved)
    \/ (phase = "Lost") /\ \E outcome \in WriterFenceAcquireOutcomeValues : \E arg_token \in 0..2 : \E lease_moved \in BOOLEAN : AcquireAnsweredLost(outcome, arg_token, lease_moved)
    \/ LeaseEnded
    \/ LeaseEndedNotWaitingOutUnopened
    \/ LeaseEndedNotWaitingOutLive
    \/ LeaseEndedNotWaitingOutStopped
    \/ LeaseEndedNotWaitingOutAcquiring
    \/ LeaseEndedNotWaitingOutProving
    \/ LeaseEndedNotWaitingOutResuming
    \/ LeaseEndedLost
    \/ (phase = "Proving") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingClean(outcome, arg_token)
    \/ (phase = "Proving") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingForeign(outcome, arg_token)
    \/ (phase = "Proving") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingFailed(outcome, arg_token)
    \/ (phase = "Proving") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingOtherGrant(outcome, arg_token)
    \/ (phase = "Unopened") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingCheckedNotProvingUnopened(outcome, arg_token)
    \/ (phase = "Live") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingCheckedNotProvingLive(outcome, arg_token)
    \/ (phase = "Stopped") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingCheckedNotProvingStopped(outcome, arg_token)
    \/ (phase = "Acquiring") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingCheckedNotProvingAcquiring(outcome, arg_token)
    \/ (phase = "Resuming") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingCheckedNotProvingResuming(outcome, arg_token)
    \/ (phase = "WaitingOut") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingCheckedNotProvingWaitingOut(outcome, arg_token)
    \/ (phase = "Lost") /\ \E outcome \in WriterFenceListingOutcomeValues : \E arg_token \in 0..2 : ListingCheckedLost(outcome, arg_token)
    \/ (phase = "Live" \/ phase = "Stopped" \/ phase = "Acquiring" \/ phase = "Proving" \/ phase = "Resuming" \/ phase = "WaitingOut") /\ \E cause \in WriterFenceLostCauseValues : Poison(cause)
    \/ (phase = "Unopened") /\ \E cause \in WriterFenceLostCauseValues : PoisonUnopened(cause)
    \/ (phase = "Lost") /\ \E cause \in WriterFenceLostCauseValues : PoisonLost(cause)
    \/ OpenListingFailedLive
    \/ OpenListingFailedStopped
    \/ OpenListingFailedAcquiring
    \/ OpenListingFailedProving
    \/ OpenListingFailedResuming
    \/ OpenListingFailedWaitingOut
    \/ OpenListingFailedUnopened
    \/ OpenListingFailedLost
    \/ (phase = "Live") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedLive(key, name, version, commit_ids)
    \/ (phase = "Stopped") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedStopped(key, name, version, commit_ids)
    \/ (phase = "Acquiring") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedAcquiring(key, name, version, commit_ids)
    \/ (phase = "Proving") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedProving(key, name, version, commit_ids)
    \/ (phase = "Resuming") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedResuming(key, name, version, commit_ids)
    \/ (phase = "WaitingOut") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedWaitingOut(key, name, version, commit_ids)
    \/ (phase = "Live") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedAgainLive(key, name, version, commit_ids)
    \/ (phase = "Stopped") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedAgainStopped(key, name, version, commit_ids)
    \/ (phase = "Acquiring") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedAgainAcquiring(key, name, version, commit_ids)
    \/ (phase = "Proving") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedAgainProving(key, name, version, commit_ids)
    \/ (phase = "Resuming") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedAgainResuming(key, name, version, commit_ids)
    \/ (phase = "WaitingOut") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedAgainWaitingOut(key, name, version, commit_ids)
    \/ (phase = "Live") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedPoisonedLive(key, name, version, commit_ids)
    \/ (phase = "Stopped") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedPoisonedStopped(key, name, version, commit_ids)
    \/ (phase = "Acquiring") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedPoisonedAcquiring(key, name, version, commit_ids)
    \/ (phase = "Proving") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedPoisonedProving(key, name, version, commit_ids)
    \/ (phase = "Resuming") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedPoisonedResuming(key, name, version, commit_ids)
    \/ (phase = "WaitingOut") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedPoisonedWaitingOut(key, name, version, commit_ids)
    \/ (phase = "Unopened") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedUnopened(key, name, version, commit_ids)
    \/ (phase = "Lost") /\ \E key \in StringValues : \E name \in StringValues : \E version \in 0..2 : \E commit_ids \in SetOfStringValues : ScopeQuarantinedLost(key, name, version, commit_ids)
    \/ (phase = "Live") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedLive(scope, cause)
    \/ (phase = "Stopped") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedStopped(scope, cause)
    \/ (phase = "Acquiring") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedAcquiring(scope, cause)
    \/ (phase = "Proving") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedProving(scope, cause)
    \/ (phase = "Resuming") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedResuming(scope, cause)
    \/ (phase = "WaitingOut") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedWaitingOut(scope, cause)
    \/ (phase = "Live") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedAgainLive(scope, cause)
    \/ (phase = "Stopped") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedAgainStopped(scope, cause)
    \/ (phase = "Acquiring") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedAgainAcquiring(scope, cause)
    \/ (phase = "Proving") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedAgainProving(scope, cause)
    \/ (phase = "Resuming") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedAgainResuming(scope, cause)
    \/ (phase = "WaitingOut") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedAgainWaitingOut(scope, cause)
    \/ (phase = "Live") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedQuarantinedLive(scope, cause)
    \/ (phase = "Stopped") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedQuarantinedStopped(scope, cause)
    \/ (phase = "Acquiring") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedQuarantinedAcquiring(scope, cause)
    \/ (phase = "Proving") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedQuarantinedProving(scope, cause)
    \/ (phase = "Resuming") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedQuarantinedResuming(scope, cause)
    \/ (phase = "WaitingOut") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedQuarantinedWaitingOut(scope, cause)
    \/ (phase = "Unopened") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedUnopened(scope, cause)
    \/ (phase = "Lost") /\ \E scope \in StringValues : \E cause \in ScopePoisonCauseValues : ScopePoisonedLost(scope, cause)
    \/ TerminalStutter

unopened_holds_nothing == (IF (phase # "Unopened") THEN TRUE ELSE ((token = 0) /\ (granted = None) /\ (own_grant = None) /\ (timed_out = 0) /\ (waited_holder = None) /\ ~(writer_stopped) /\ ~(open_listing_failed) /\ (Cardinality(quarantined_keys) = 0) /\ (Cardinality(DOMAIN quarantine_versions) = 0) /\ (Cardinality(DOMAIN poisoned_scopes) = 0)))
no_scope_is_quarantined_and_poisoned == (\A key \in quarantined_keys : ~((key \in DOMAIN poisoned_scopes)))
quarantine_records_are_whole == (DOMAIN quarantine_versions = DOMAIN quarantine_commit_ids)
live_writer_is_not_stopped == (IF (phase # "Live") THEN TRUE ELSE ~(writer_stopped))
stopped_and_attempting_writer_is_stopped == (IF ((phase # "Stopped") /\ (phase # "Acquiring") /\ (phase # "Proving") /\ (phase # "Resuming") /\ (phase # "WaitingOut")) THEN TRUE ELSE writer_stopped)
proving_and_resuming_carry_their_grant == (IF ((phase # "Proving") /\ (phase # "Resuming")) THEN TRUE ELSE (granted # None))
grant_carried_only_while_proving_or_resuming == (IF (granted = None) THEN TRUE ELSE (IF (phase = "Proving") THEN TRUE ELSE (phase = "Resuming")))
own_grant_is_above_the_token == (IF (own_grant = None) THEN TRUE ELSE ((IF "value" \in DOMAIN own_grant THEN own_grant["value"] ELSE None) > token))
waiting_out_follows_a_timed_out_acquire == (IF (phase # "WaitingOut") THEN TRUE ELSE ((timed_out > 0) /\ (waited_holder # None)))
waited_holder_needs_a_timed_out_acquire == (IF (waited_holder = None) THEN TRUE ELSE (timed_out > 0))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(quarantined_keys) <= 1 /\ Cardinality(DOMAIN quarantine_versions) <= 1 /\ Cardinality(DOMAIN quarantine_commit_ids) <= 1 /\ Cardinality(DOMAIN poisoned_scopes) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(quarantined_keys) <= 2 /\ Cardinality(DOMAIN quarantine_versions) <= 2 /\ Cardinality(DOMAIN quarantine_commit_ids) <= 2 /\ Cardinality(DOMAIN poisoned_scopes) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []unopened_holds_nothing
THEOREM Spec => []no_scope_is_quarantined_and_poisoned
THEOREM Spec => []quarantine_records_are_whole
THEOREM Spec => []live_writer_is_not_stopped
THEOREM Spec => []stopped_and_attempting_writer_is_stopped
THEOREM Spec => []proving_and_resuming_carry_their_grant
THEOREM Spec => []grant_carried_only_while_proving_or_resuming
THEOREM Spec => []own_grant_is_above_the_token
THEOREM Spec => []waiting_out_follows_a_timed_out_acquire
THEOREM Spec => []waited_holder_needs_a_timed_out_acquire

=============================================================================
