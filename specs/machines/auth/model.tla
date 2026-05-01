---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for AuthMachine.

CONSTANTS BooleanValues, NatValues, SetOfStringValues, StringValues

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
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids

vars == << phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>

Init ==
    /\ phase = "Valid"
    /\ model_step_count = 0
    /\ expires_at = None
    /\ last_refresh = None
    /\ refresh_attempt = 0
    /\ credential_present = FALSE
    /\ oauth_browser_flow_ids = {}
    /\ oauth_device_flow_ids = {}
    /\ oauth_device_poll_ids = {}

TerminalStutter ==
    /\ phase = "Released"
    /\ UNCHANGED vars

Acquire(expires_at_ts) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = expires_at_ts
    /\ refresh_attempt' = 0
    /\ credential_present' = TRUE
    /\ UNCHANGED << last_refresh, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


MarkExpiring ==
    /\ phase = "Valid"
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


BeginRefreshFromValid ==
    /\ phase = "Valid"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


BeginRefreshFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


CompleteRefresh(new_expires_at, now_ts) ==
    /\ phase = "Refreshing"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = new_expires_at
    /\ last_refresh' = Some(now_ts)
    /\ refresh_attempt' = 0
    /\ UNCHANGED << credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


RefreshFailedTransient ==
    /\ phase = "Refreshing"
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UNCHANGED << expires_at, last_refresh, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


RefreshFailedPermanent ==
    /\ phase = "Refreshing"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UNCHANGED << expires_at, last_refresh, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


MarkReauthRequiredFromValid ==
    /\ phase = "Valid"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


MarkReauthRequiredFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


MarkReauthRequiredFromRefreshing ==
    /\ phase = "Refreshing"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


ClearCredentialLifecycle ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


Release ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ credential_present' = FALSE
    /\ oauth_browser_flow_ids' = {}
    /\ oauth_device_flow_ids' = {}
    /\ oauth_device_poll_ids' = {}
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt >>


AdmitOAuthBrowserFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


AdmitOAuthBrowserFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


AdmitOAuthBrowserFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


AdmitOAuthBrowserFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


VerifyOAuthBrowserFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


VerifyOAuthBrowserFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


VerifyOAuthBrowserFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


VerifyOAuthBrowserFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_device_flow_ids, oauth_device_poll_ids >>


AdmitOAuthDeviceFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


AdmitOAuthDeviceFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


AdmitOAuthDeviceFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


AdmitOAuthDeviceFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


VerifyOAuthDeviceFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


VerifyOAuthDeviceFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


VerifyOAuthDeviceFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


VerifyOAuthDeviceFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids, oauth_device_poll_ids >>


BeginOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


BeginOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


BeginOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


BeginOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


FinishOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


FinishOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


FinishOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


FinishOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_device_flow_ids >>


ConsumeOAuthDeviceFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


ConsumeOAuthDeviceFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


ConsumeOAuthDeviceFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


ConsumeOAuthDeviceFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


ExpireOAuthDeviceFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


ExpireOAuthDeviceFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


ExpireOAuthDeviceFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


ExpireOAuthDeviceFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids >>


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
    \/ ClearCredentialLifecycle
    \/ Release
    \/ \E flow_id \in StringValues : AdmitOAuthBrowserFlowValid(flow_id)
    \/ \E flow_id \in StringValues : AdmitOAuthBrowserFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : AdmitOAuthBrowserFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : AdmitOAuthBrowserFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthBrowserFlowValid(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthBrowserFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthBrowserFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthBrowserFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthBrowserFlowValid(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthBrowserFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthBrowserFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthBrowserFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowValid(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : AdmitOAuthDeviceFlowValid(flow_id)
    \/ \E flow_id \in StringValues : AdmitOAuthDeviceFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : AdmitOAuthDeviceFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : AdmitOAuthDeviceFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthDeviceFlowValid(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthDeviceFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthDeviceFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : VerifyOAuthDeviceFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : BeginOAuthDevicePollValid(flow_id)
    \/ \E flow_id \in StringValues : BeginOAuthDevicePollExpiring(flow_id)
    \/ \E flow_id \in StringValues : BeginOAuthDevicePollRefreshing(flow_id)
    \/ \E flow_id \in StringValues : BeginOAuthDevicePollReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollValid(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollExpiring(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollRefreshing(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthDeviceFlowValid(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthDeviceFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthDeviceFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : ConsumeOAuthDeviceFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowValid(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowReauthRequired(flow_id)
    \/ TerminalStutter


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(oauth_browser_flow_ids) <= 1 /\ Cardinality(oauth_device_flow_ids) <= 1 /\ Cardinality(oauth_device_poll_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(oauth_browser_flow_ids) <= 2 /\ Cardinality(oauth_device_flow_ids) <= 2 /\ Cardinality(oauth_device_poll_ids) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
