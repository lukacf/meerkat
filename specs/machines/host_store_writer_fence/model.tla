---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for HostStoreWriterFenceMachine.

CONSTANTS BooleanValues, NatValues, WriterFenceAcquireOutcomeValues, WriterFenceAcquireStepValues, WriterFenceListingOutcomeValues, WriterFenceLostCauseValues, WriterFenceRefusalValues, WriterFenceRenewOutcomeValues, WriterFenceStopCauseValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

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

VARIABLES phase, model_step_count, token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped

vars == << phase, model_step_count, token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped >>

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

TerminalStutter ==
    /\ phase = "Lost"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_0b7d97d0c8a3e6a8 == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, lost_cause >>
UnchangedFrame_27ae1a2ba7929ceb == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped >>
UnchangedFrame_321073f37d2d1075 == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, stop_cause, writer_stopped >>
UnchangedFrame_3c687f0506a92472 == UNCHANGED << token, own_grant, step, timed_out, waited_holder, lost_cause, writer_stopped >>
UnchangedFrame_3e297d97d64c5619 == UNCHANGED << granted, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped >>
UnchangedFrame_4653b2b4b5384b44 == UNCHANGED << token, own_grant, step, timed_out, waited_holder, stop_cause, writer_stopped >>
UnchangedFrame_5a6cc2b62c1a3af9 == UNCHANGED << token, granted, own_grant, step, waited_holder, lost_cause, writer_stopped >>
UnchangedFrame_b1ccba944fdc0189 == UNCHANGED << token, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause >>
UnchangedFrame_bf8265ad4b2a1ee3 == UNCHANGED << token, granted, own_grant, step, timed_out, waited_holder, lost_cause, writer_stopped >>
UnchangedFrame_e3f1e6fb26f3c3c2 == UNCHANGED << token, step, stop_cause, lost_cause, writer_stopped >>
UnchangedFrame_eb3505c1b52ba7b0 == UNCHANGED << granted, own_grant, step, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped >>
UnchangedFrame_f34d36f620f6c1b5 == UNCHANGED << token, granted, own_grant, timed_out, waited_holder, stop_cause, lost_cause, writer_stopped >>
UnchangedFrame_fe6e3a1e56d7caf8 == UNCHANGED << token, granted, own_grant, step, timed_out, stop_cause, lost_cause, writer_stopped >>

Open(arg_token) ==
    /\ phase = "Unopened"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ token' = arg_token
    /\ UnchangedFrame_eb3505c1b52ba7b0


OpenAgainLive(arg_token) ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenAgainStopped(arg_token) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenAgainAcquiring(arg_token) ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenAgainProving(arg_token) ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenAgainResuming(arg_token) ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenAgainWaitingOut(arg_token) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenLost(arg_token) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


WindowClosedLive ==
    /\ phase = "Live"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "WindowClosed"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_0b7d97d0c8a3e6a8


WindowClosedNotLiveStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


WindowClosedNotLiveAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


WindowClosedNotLiveProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


WindowClosedNotLiveResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


WindowClosedNotLiveWaitingOut ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


WindowClosedUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


WindowClosedLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewedInWindow(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "Renewed")
    /\ in_window
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewedAfterTheWindow(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "Renewed")
    /\ ~(in_window)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "WindowClosedBeforeRenew"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_0b7d97d0c8a3e6a8


RenewStaleLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "FencedOut")
    /\ ((current # renewed) /\ (current = token))
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewFencedOutLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "FencedOut")
    /\ (IF (current = renewed) THEN TRUE ELSE (current # token))
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "FencedOutRenew"
    /\ UnchangedFrame_321073f37d2d1075


RenewFailedLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "RenewFailed"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_0b7d97d0c8a3e6a8


RenewTimedOutLive(outcome, renewed, current, in_window) ==
    /\ phase = "Live"
    /\ (outcome = "TimedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "RenewTimedOut"
    /\ writer_stopped' = TRUE
    /\ UnchangedFrame_0b7d97d0c8a3e6a8


RenewAnsweredStopped(outcome, renewed, current, in_window) ==
    /\ phase = "Stopped"
    /\ (outcome # "FencedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewStaleStopped(outcome, renewed, current, in_window) ==
    /\ phase = "Stopped"
    /\ (outcome = "FencedOut")
    /\ ((current # renewed) /\ (current = token))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewFencedOutStopped(outcome, renewed, current, in_window) ==
    /\ phase = "Stopped"
    /\ (outcome = "FencedOut")
    /\ (IF (current = renewed) THEN TRUE ELSE (current # token))
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "FencedOutRenew"
    /\ UnchangedFrame_321073f37d2d1075


ResumeRenewed(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "Renewed")
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ writer_stopped' = FALSE
    /\ UnchangedFrame_b1ccba944fdc0189


ResumeFencedOut(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "FencedOut")
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ lost_cause' = "FencedOutAfterReacquire"
    /\ UnchangedFrame_4653b2b4b5384b44


ResumeRenewFailed(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ stop_cause' = "ResumeRenewFailed"
    /\ UnchangedFrame_3c687f0506a92472


ResumeRenewTimedOut(outcome, renewed, current, in_window) ==
    /\ phase = "Resuming"
    /\ (outcome = "TimedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ stop_cause' = "ResumeRenewTimedOut"
    /\ UnchangedFrame_3c687f0506a92472


RenewAnsweredNoRenewUnopened(outcome, renewed, current, in_window) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewAnsweredNoRenewAcquiring(outcome, renewed, current, in_window) ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewAnsweredNoRenewProving(outcome, renewed, current, in_window) ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewAnsweredNoRenewWaitingOut(outcome, renewed, current, in_window) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


RenewAnsweredLost(outcome, renewed, current, in_window) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


BeginReacquire ==
    /\ phase = "Stopped"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ step' = "Exclusive"
    /\ UnchangedFrame_f34d36f620f6c1b5


BeginReacquireLive ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


BeginReacquireInFlightAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


BeginReacquireInFlightProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


BeginReacquireInFlightResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


BeginReacquireWaitingOut ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


BeginReacquireUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


BeginReacquireLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


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
    /\ UnchangedFrame_e3f1e6fb26f3c3c2


AcquireGrantedOutOfOrder(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Granted")
    /\ (arg_token <= token)
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "TokenOrder"
    /\ UnchangedFrame_321073f37d2d1075


AcquireHeldByOwnLease(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "Exclusive")
    /\ (IF (arg_token = token) THEN TRUE ELSE (own_grant = Some(arg_token)))
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ step' = "TakeoverOwn"
    /\ UnchangedFrame_f34d36f620f6c1b5


AcquireHeldByAnother(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "Exclusive")
    /\ ((arg_token # token) /\ (own_grant # Some(arg_token)))
    /\ (timed_out = 0)
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ lost_cause' = "HeldByAnother"
    /\ UnchangedFrame_321073f37d2d1075


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
    /\ UnchangedFrame_321073f37d2d1075


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
    /\ UnchangedFrame_27ae1a2ba7929ceb


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
    /\ UnchangedFrame_fe6e3a1e56d7caf8


AcquireHeldAtTakeover(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Held")
    /\ (step = "TakeoverOwn")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "ReacquireFailed"
    /\ UnchangedFrame_bf8265ad4b2a1ee3


AcquireFailed(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ stop_cause' = "ReacquireFailed"
    /\ UnchangedFrame_bf8265ad4b2a1ee3


AcquireTimedOut(outcome, arg_token, lease_moved) ==
    /\ phase = "Acquiring"
    /\ (outcome = "TimedOut")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ timed_out' = (timed_out) + 1
    /\ stop_cause' = "ReacquireTimedOut"
    /\ UnchangedFrame_5a6cc2b62c1a3af9


AcquireAnsweredNotAcquiringUnopened(outcome, arg_token, lease_moved) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


AcquireAnsweredNotAcquiringLive(outcome, arg_token, lease_moved) ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


AcquireAnsweredNotAcquiringStopped(outcome, arg_token, lease_moved) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


AcquireAnsweredNotAcquiringProving(outcome, arg_token, lease_moved) ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


AcquireAnsweredNotAcquiringResuming(outcome, arg_token, lease_moved) ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


AcquireAnsweredNotAcquiringWaitingOut(outcome, arg_token, lease_moved) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


AcquireAnsweredLost(outcome, arg_token, lease_moved) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEnded ==
    /\ phase = "WaitingOut"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEndedNotWaitingOutUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEndedNotWaitingOutLive ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEndedNotWaitingOutStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEndedNotWaitingOutAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEndedNotWaitingOutProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEndedNotWaitingOutResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


LeaseEndedLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingClean(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted = Some(arg_token))
    /\ (outcome = "Clean")
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ token' = arg_token
    /\ own_grant' = None
    /\ UnchangedFrame_3e297d97d64c5619


ListingForeign(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted = Some(arg_token))
    /\ (outcome = "ForeignBatch")
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ lost_cause' = "ForeignBatch"
    /\ UnchangedFrame_4653b2b4b5384b44


ListingFailed(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted = Some(arg_token))
    /\ (outcome = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ stop_cause' = "ListingFailed"
    /\ UnchangedFrame_3c687f0506a92472


ListingOtherGrant(outcome, arg_token) ==
    /\ phase = "Proving"
    /\ (granted # Some(arg_token))
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingCheckedNotProvingUnopened(outcome, arg_token) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingCheckedNotProvingLive(outcome, arg_token) ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingCheckedNotProvingStopped(outcome, arg_token) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingCheckedNotProvingAcquiring(outcome, arg_token) ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingCheckedNotProvingResuming(outcome, arg_token) ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingCheckedNotProvingWaitingOut(outcome, arg_token) ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


ListingCheckedLost(outcome, arg_token) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


Poison(cause) ==
    /\ phase = "Live" \/ phase = "Stopped" \/ phase = "Acquiring" \/ phase = "Proving" \/ phase = "Resuming" \/ phase = "WaitingOut"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ granted' = None
    /\ lost_cause' = cause
    /\ UnchangedFrame_4653b2b4b5384b44


PoisonUnopened(cause) ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


PoisonLost(cause) ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedLive ==
    /\ phase = "Live"
    /\ phase' = "Live"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedAcquiring ==
    /\ phase = "Acquiring"
    /\ phase' = "Acquiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedProving ==
    /\ phase = "Proving"
    /\ phase' = "Proving"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedResuming ==
    /\ phase = "Resuming"
    /\ phase' = "Resuming"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedWaitingOut ==
    /\ phase = "WaitingOut"
    /\ phase' = "WaitingOut"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedUnopened ==
    /\ phase = "Unopened"
    /\ phase' = "Unopened"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


OpenListingFailedLost ==
    /\ phase = "Lost"
    /\ phase' = "Lost"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_27ae1a2ba7929ceb


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
    \/ TerminalStutter

unopened_holds_nothing == (IF (phase # "Unopened") THEN TRUE ELSE ((token = 0) /\ (granted = None) /\ (own_grant = None) /\ (timed_out = 0) /\ (waited_holder = None) /\ ~(writer_stopped)))
live_writer_is_not_stopped == (IF (phase # "Live") THEN TRUE ELSE ~(writer_stopped))
stopped_and_attempting_writer_is_stopped == (IF ((phase # "Stopped") /\ (phase # "Acquiring") /\ (phase # "Proving") /\ (phase # "Resuming") /\ (phase # "WaitingOut")) THEN TRUE ELSE writer_stopped)
proving_and_resuming_carry_their_grant == (IF ((phase # "Proving") /\ (phase # "Resuming")) THEN TRUE ELSE (granted # None))
grant_carried_only_while_proving_or_resuming == (IF (granted = None) THEN TRUE ELSE (IF (phase = "Proving") THEN TRUE ELSE (phase = "Resuming")))
own_grant_is_above_the_token == (IF (own_grant = None) THEN TRUE ELSE ((IF "value" \in DOMAIN own_grant THEN own_grant["value"] ELSE None) > token))
waiting_out_follows_a_timed_out_acquire == (IF (phase # "WaitingOut") THEN TRUE ELSE ((timed_out > 0) /\ (waited_holder # None)))
waited_holder_needs_a_timed_out_acquire == (IF (waited_holder = None) THEN TRUE ELSE (timed_out > 0))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []unopened_holds_nothing
THEOREM Spec => []live_writer_is_not_stopped
THEOREM Spec => []stopped_and_attempting_writer_is_stopped
THEOREM Spec => []proving_and_resuming_carry_their_grant
THEOREM Spec => []grant_carried_only_while_proving_or_resuming
THEOREM Spec => []own_grant_is_above_the_token
THEOREM Spec => []waiting_out_follows_a_timed_out_acquire
THEOREM Spec => []waited_holder_needs_a_timed_out_acquire

=============================================================================
