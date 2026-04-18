---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for AuthMachine.

CONSTANTS NatValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, expires_at, last_refresh, refresh_attempt

vars == << phase, model_step_count, expires_at, last_refresh, refresh_attempt >>

Init ==
    /\ phase = "Valid"
    /\ model_step_count = 0
    /\ expires_at = None
    /\ last_refresh = None
    /\ refresh_attempt = 0

TerminalStutter ==
    /\ phase = "Released"
    /\ UNCHANGED vars

Acquire(expires_at_ts) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = expires_at_ts
    /\ refresh_attempt' = 0
    /\ UNCHANGED << last_refresh >>


MarkExpiring ==
    /\ phase = "Valid"
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


BeginRefreshFromValid ==
    /\ phase = "Valid"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


BeginRefreshFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


CompleteRefresh(new_expires_at, now_ts) ==
    /\ phase = "Refreshing"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = new_expires_at
    /\ last_refresh' = Some(now_ts)
    /\ refresh_attempt' = 0


RefreshFailedTransient ==
    /\ phase = "Refreshing"
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UNCHANGED << expires_at, last_refresh >>


RefreshFailedPermanent ==
    /\ phase = "Refreshing"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UNCHANGED << expires_at, last_refresh >>


MarkReauthRequiredFromValid ==
    /\ phase = "Valid"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


MarkReauthRequiredFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


MarkReauthRequiredFromRefreshing ==
    /\ phase = "Refreshing"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


Release ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


Next ==
    \/ \E expires_at_ts \in OptionU64Values : Acquire(expires_at_ts)
    \/ MarkExpiring
    \/ BeginRefreshFromValid
    \/ BeginRefreshFromExpiring
    \/ \E new_expires_at \in OptionU64Values : \E now_ts \in 0..2 : CompleteRefresh(new_expires_at, now_ts)
    \/ RefreshFailedTransient
    \/ RefreshFailedPermanent
    \/ MarkReauthRequiredFromValid
    \/ MarkReauthRequiredFromExpiring
    \/ MarkReauthRequiredFromRefreshing
    \/ Release
    \/ TerminalStutter


CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars


=============================================================================
