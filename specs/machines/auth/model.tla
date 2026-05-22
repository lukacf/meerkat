---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for AuthMachine.

CONSTANTS AuthLifecyclePhaseValues, BooleanValues, NatValues, SetOfStringValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in StringValues }
MapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in NatValues }
OptionAuthLifecyclePhaseValues == {None} \cup {Some(x) : x \in AuthLifecyclePhaseValues}
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

VARIABLES phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count

vars == << phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>

Init ==
    /\ phase = "Valid"
    /\ model_step_count = 0
    /\ expires_at = None
    /\ last_refresh = None
    /\ refresh_attempt = 0
    /\ credential_present = FALSE
    /\ credential_generation = 0
    /\ credential_published_at_millis = None
    /\ oauth_browser_flow_ids = {}
    /\ oauth_browser_flow_providers = [x \in {} |-> None]
    /\ oauth_browser_flow_redirect_uris = [x \in {} |-> None]
    /\ oauth_browser_flow_expires_at_millis = [x \in {} |-> None]
    /\ oauth_device_flow_ids = {}
    /\ oauth_device_flow_providers = [x \in {} |-> None]
    /\ oauth_device_flow_expires_at_millis = [x \in {} |-> None]
    /\ oauth_device_poll_ids = {}
    /\ oauth_outstanding_flow_count = 0

TerminalStutter ==
    /\ phase = "Released"
    /\ UNCHANGED vars

Acquire(expires_at_ts, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = expires_at_ts
    /\ refresh_attempt' = 0
    /\ credential_present' = TRUE
    /\ credential_generation' = (credential_generation + 1)
    /\ credential_published_at_millis' = Some(arg_credential_published_at_millis)
    /\ UNCHANGED << last_refresh, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


MarkExpiring ==
    /\ phase = "Valid"
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN TRUE ELSE ((now_ts + refresh_window_secs) <= (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessExpiringFromValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)) /\ ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) < (now_ts + refresh_window_secs))))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessExpiredFromValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) <= now_ts))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessExpiring(now_ts, refresh_window_secs) ==
    /\ phase = "Expiring"
    /\ (IF (expires_at = None) THEN TRUE ELSE (now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessExpiredFromExpiring(now_ts, refresh_window_secs) ==
    /\ phase = "Expiring"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) <= now_ts))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessExpired(now_ts, refresh_window_secs) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessRefreshing(now_ts, refresh_window_secs) ==
    /\ phase = "Refreshing"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessReauthRequired(now_ts, refresh_window_secs) ==
    /\ phase = "ReauthRequired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ObserveCredentialFreshnessReleased(now_ts, refresh_window_secs) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


BeginRefreshFromValid ==
    /\ phase = "Valid"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


BeginRefreshFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


BeginRefreshFromExpired ==
    /\ phase = "Expired"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


CompleteRefresh(new_expires_at, now_ts, arg_credential_published_at_millis) ==
    /\ phase = "Refreshing"
    /\ (IF (new_expires_at = None) THEN TRUE ELSE (now_ts < (IF "value" \in DOMAIN new_expires_at THEN new_expires_at["value"] ELSE None)))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = new_expires_at
    /\ last_refresh' = Some(now_ts)
    /\ refresh_attempt' = 0
    /\ credential_present' = TRUE
    /\ credential_generation' = (credential_generation + 1)
    /\ credential_published_at_millis' = Some(arg_credential_published_at_millis)
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RefreshFailedTransient(http_status, oauth_error_code, local_credential_unusable) ==
    /\ phase = "Refreshing"
    /\ ((local_credential_unusable = FALSE) /\ (http_status # Some(401)) /\ (http_status # Some(403)) /\ (oauth_error_code # Some("invalid_grant")) /\ (oauth_error_code # Some("invalid_client")) /\ (oauth_error_code # Some("unauthorized_client")) /\ (oauth_error_code # Some("invalid_scope")) /\ (oauth_error_code # Some("access_denied")) /\ (oauth_error_code # Some("permission_denied")) /\ (oauth_error_code # Some("expired_token")))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UNCHANGED << expires_at, last_refresh, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RefreshFailedPermanent(http_status, oauth_error_code, local_credential_unusable) ==
    /\ phase = "Refreshing"
    /\ ((local_credential_unusable = TRUE) \/ (http_status = Some(401)) \/ (http_status = Some(403)) \/ (oauth_error_code = Some("invalid_grant")) \/ (oauth_error_code = Some("invalid_client")) \/ (oauth_error_code = Some("unauthorized_client")) \/ (oauth_error_code = Some("invalid_scope")) \/ (oauth_error_code = Some("access_denied")) \/ (oauth_error_code = Some("permission_denied")) \/ (oauth_error_code = Some("expired_token")))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UNCHANGED << expires_at, last_refresh, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


MarkReauthRequiredFromValid ==
    /\ phase = "Valid"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


MarkReauthRequiredFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


MarkReauthRequiredFromExpired ==
    /\ phase = "Expired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


MarkReauthRequiredFromRefreshing ==
    /\ phase = "Refreshing"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ClearCredentialLifecycle ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_published_at_millis' = None
    /\ UNCHANGED << credential_generation, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ReleaseCredentialLifecycleWithOAuth ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_published_at_millis' = None
    /\ UNCHANGED << credential_generation, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ReleaseCredentialLifecycleWithoutOAuth ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_published_at_millis' = None
    /\ oauth_browser_flow_ids' = {}
    /\ oauth_browser_flow_providers' = [x \in {} |-> None]
    /\ oauth_browser_flow_redirect_uris' = [x \in {} |-> None]
    /\ oauth_browser_flow_expires_at_millis' = [x \in {} |-> None]
    /\ oauth_device_flow_ids' = {}
    /\ oauth_device_flow_providers' = [x \in {} |-> None]
    /\ oauth_device_flow_expires_at_millis' = [x \in {} |-> None]
    /\ oauth_device_poll_ids' = {}
    /\ oauth_outstanding_flow_count' = 0
    /\ UNCHANGED << credential_generation >>


Release ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_published_at_millis' = None
    /\ oauth_browser_flow_ids' = {}
    /\ oauth_browser_flow_providers' = [x \in {} |-> None]
    /\ oauth_browser_flow_redirect_uris' = [x \in {} |-> None]
    /\ oauth_browser_flow_expires_at_millis' = [x \in {} |-> None]
    /\ oauth_device_flow_ids' = {}
    /\ oauth_device_flow_providers' = [x \in {} |-> None]
    /\ oauth_device_flow_expires_at_millis' = [x \in {} |-> None]
    /\ oauth_device_poll_ids' = {}
    /\ oauth_outstanding_flow_count' = 0
    /\ UNCHANGED << credential_generation >>


RestoreCredentialLifecycleSnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Valid")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreCredentialLifecycleSnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Expiring")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreCredentialLifecycleSnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Refreshing")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreCredentialLifecycleSnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Expired")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreCredentialLifecycleSnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("ReauthRequired")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((arg_credential_present = FALSE) \/ (lifecycle_phase = None) \/ (lifecycle_phase = Some("Released")))
    /\ ((oauth_outstanding_flow_count > 0) \/ restored_oauth_membership_observed)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = None
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((arg_credential_present = FALSE) \/ (lifecycle_phase = None) \/ (lifecycle_phase = Some("Released")))
    /\ ((oauth_outstanding_flow_count = 0) /\ (restored_oauth_membership_observed = FALSE))
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = None
    /\ oauth_browser_flow_ids' = {}
    /\ oauth_browser_flow_providers' = [x \in {} |-> None]
    /\ oauth_browser_flow_redirect_uris' = [x \in {} |-> None]
    /\ oauth_browser_flow_expires_at_millis' = [x \in {} |-> None]
    /\ oauth_device_flow_ids' = {}
    /\ oauth_device_flow_providers' = [x \in {} |-> None]
    /\ oauth_device_flow_expires_at_millis' = [x \in {} |-> None]
    /\ oauth_device_poll_ids' = {}
    /\ oauth_outstanding_flow_count' = 0


RestoreAuthoritySnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = "Valid") /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreAuthoritySnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = "Expiring") /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreAuthoritySnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = "Refreshing") /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreAuthoritySnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = "Expired") /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreAuthoritySnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = "ReauthRequired") /\ ((arg_credential_present = FALSE) \/ (arg_credential_published_at_millis # None)))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreAuthoritySnapshotReleased(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = "Released") /\ (arg_credential_present = FALSE) /\ (arg_credential_published_at_millis = None) /\ (oauth_outstanding_flow_count = 0))
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


RestoreOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Valid"
    /\ (provider # None)
    /\ (redirect_uri # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, (IF "value" \in DOMAIN redirect_uri THEN redirect_uri["value"] ELSE None))
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_browser_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


RestoreOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Expiring"
    /\ (provider # None)
    /\ (redirect_uri # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, (IF "value" \in DOMAIN redirect_uri THEN redirect_uri["value"] ELSE None))
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_browser_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


RestoreOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Expired"
    /\ (provider # None)
    /\ (redirect_uri # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, (IF "value" \in DOMAIN redirect_uri THEN redirect_uri["value"] ELSE None))
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_browser_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


RestoreOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Refreshing"
    /\ (provider # None)
    /\ (redirect_uri # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, (IF "value" \in DOMAIN redirect_uri THEN redirect_uri["value"] ELSE None))
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_browser_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


RestoreOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "ReauthRequired"
    /\ (provider # None)
    /\ (redirect_uri # None)
    /\ (expires_at_millis # None)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, (IF "value" \in DOMAIN redirect_uri THEN redirect_uri["value"] ELSE None))
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_browser_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


RestoreOAuthDeviceFlowValid(flow_id, provider, expires_at_millis) ==
    /\ phase = "Valid"
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


RestoreOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis) ==
    /\ phase = "Expiring"
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


RestoreOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis) ==
    /\ phase = "Expired"
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


RestoreOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis) ==
    /\ phase = "Refreshing"
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


RestoreOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis) ==
    /\ phase = "ReauthRequired"
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


RestoreOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


RestoreOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


RestoreOAuthDevicePollExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


RestoreOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


RestoreOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


AdmitOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, provider)
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, redirect_uri)
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


AdmitOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, provider)
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, redirect_uri)
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


AdmitOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, provider)
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, redirect_uri)
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


AdmitOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, provider)
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, redirect_uri)
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


AdmitOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, provider)
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, redirect_uri)
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ReopenReleasedForOAuthBrowserFlowAdmission(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Released"
    /\ ((credential_present = FALSE) /\ (credential_published_at_millis = None))
    /\ (oauth_outstanding_flow_count = 0)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \cup {flow_id})
    /\ oauth_browser_flow_providers' = MapSet(oauth_browser_flow_providers, flow_id, provider)
    /\ oauth_browser_flow_redirect_uris' = MapSet(oauth_browser_flow_redirect_uris, flow_id, redirect_uri)
    /\ oauth_browser_flow_expires_at_millis' = MapSet(oauth_browser_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


VerifyOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ConsumeOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ConsumeOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


ExpireOAuthBrowserFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids >>


AdmitOAuthDeviceFlowValid(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, provider)
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


AdmitOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, provider)
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


AdmitOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, provider)
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


AdmitOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, provider)
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


AdmitOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, provider)
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ReopenReleasedForOAuthDeviceFlowAdmission(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Released"
    /\ ((credential_present = FALSE) /\ (credential_published_at_millis = None))
    /\ (oauth_outstanding_flow_count = 0)
    /\ (oauth_outstanding_flow_count < max_outstanding_flows)
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, provider)
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, expires_at_millis)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count + 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ConfirmOAuthDurableAdmissionValid(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Valid"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ConfirmOAuthDurableAdmissionExpiring(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Expiring"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ConfirmOAuthDurableAdmissionExpired(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Expired"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ConfirmOAuthDurableAdmissionRefreshing(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Refreshing"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


ConfirmOAuthDurableAdmissionReauthRequired(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "ReauthRequired"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthDeviceFlowValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthDeviceFlowExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthDeviceFlowExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthDeviceFlowRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


VerifyOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>


BeginOAuthDevicePollValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


BeginOAuthDevicePollExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


BeginOAuthDevicePollExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


BeginOAuthDevicePollRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


BeginOAuthDevicePollReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


FinishOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


FinishOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


FinishOAuthDevicePollExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


FinishOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


FinishOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count >>


ConsumeOAuthDeviceFlowValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ConsumeOAuthDeviceFlowExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ConsumeOAuthDeviceFlowExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ConsumeOAuthDeviceFlowRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ConsumeOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ExpireOAuthDeviceFlowValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ExpireOAuthDeviceFlowExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ExpireOAuthDeviceFlowExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ExpireOAuthDeviceFlowRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


ExpireOAuthDeviceFlowReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis >>


Next ==
    \/ \E expires_at_ts \in OptionU64Values : \E arg_credential_published_at_millis \in 0..2 : Acquire(expires_at_ts, arg_credential_published_at_millis)
    \/ MarkExpiring
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessValid(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiringFromValid(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiredFromValid(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiring(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiredFromExpiring(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpired(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessRefreshing(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessReauthRequired(now_ts, refresh_window_secs)
    \/ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessReleased(now_ts, refresh_window_secs)
    \/ BeginRefreshFromValid
    \/ BeginRefreshFromExpiring
    \/ BeginRefreshFromExpired
    \/ \E new_expires_at \in OptionU64Values : \E now_ts \in 0..2 : \E arg_credential_published_at_millis \in 0..2 : CompleteRefresh(new_expires_at, now_ts, arg_credential_published_at_millis)
    \/ \E http_status \in OptionU64Values : \E oauth_error_code \in OptionStringValues : \E local_credential_unusable \in BOOLEAN : RefreshFailedTransient(http_status, oauth_error_code, local_credential_unusable)
    \/ \E http_status \in OptionU64Values : \E oauth_error_code \in OptionStringValues : \E local_credential_unusable \in BOOLEAN : RefreshFailedPermanent(http_status, oauth_error_code, local_credential_unusable)
    \/ MarkReauthRequiredFromValid
    \/ MarkReauthRequiredFromExpiring
    \/ MarkReauthRequiredFromExpired
    \/ MarkReauthRequiredFromRefreshing
    \/ ClearCredentialLifecycle
    \/ ReleaseCredentialLifecycleWithOAuth
    \/ ReleaseCredentialLifecycleWithoutOAuth
    \/ Release
    \/ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotReleased(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowValid(flow_id, provider, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis)
    \/ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis)
    \/ \E flow_id \in StringValues : RestoreOAuthDevicePollValid(flow_id)
    \/ \E flow_id \in StringValues : RestoreOAuthDevicePollExpiring(flow_id)
    \/ \E flow_id \in StringValues : RestoreOAuthDevicePollExpired(flow_id)
    \/ \E flow_id \in StringValues : RestoreOAuthDevicePollRefreshing(flow_id)
    \/ \E flow_id \in StringValues : RestoreOAuthDevicePollReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : ReopenReleasedForOAuthBrowserFlowAdmission(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowValid(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowExpired(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthBrowserFlowReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowValid(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : ReopenReleasedForOAuthDeviceFlowAdmission(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionValid(observed_global_outstanding_flows, max_outstanding_flows)
    \/ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionExpiring(observed_global_outstanding_flows, max_outstanding_flows)
    \/ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionExpired(observed_global_outstanding_flows, max_outstanding_flows)
    \/ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionRefreshing(observed_global_outstanding_flows, max_outstanding_flows)
    \/ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionReauthRequired(observed_global_outstanding_flows, max_outstanding_flows)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowValid(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowExpiring(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowExpired(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowRefreshing(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollValid(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollExpiring(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollExpired(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollRefreshing(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollReauthRequired(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollValid(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollExpiring(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollExpired(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollRefreshing(flow_id)
    \/ \E flow_id \in StringValues : FinishOAuthDevicePollReauthRequired(flow_id)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowValid(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowExpiring(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowExpired(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowRefreshing(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowValid(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowExpiring(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowExpired(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowRefreshing(flow_id)
    \/ \E flow_id \in StringValues : ExpireOAuthDeviceFlowReauthRequired(flow_id)
    \/ TerminalStutter

oauth_flow_membership_consistent == ((DOMAIN oauth_browser_flow_providers = oauth_browser_flow_ids) /\ (DOMAIN oauth_browser_flow_redirect_uris = oauth_browser_flow_ids) /\ (DOMAIN oauth_browser_flow_expires_at_millis = oauth_browser_flow_ids) /\ (DOMAIN oauth_device_flow_providers = oauth_device_flow_ids) /\ (DOMAIN oauth_device_flow_expires_at_millis = oauth_device_flow_ids) /\ (\A flow_id \in oauth_device_poll_ids : (flow_id \in oauth_device_flow_ids)) /\ (oauth_outstanding_flow_count = (Cardinality(oauth_browser_flow_ids) + Cardinality(oauth_device_flow_ids))))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(oauth_browser_flow_ids) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_providers) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_redirect_uris) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_expires_at_millis) <= 1 /\ Cardinality(oauth_device_flow_ids) <= 1 /\ Cardinality(DOMAIN oauth_device_flow_providers) <= 1 /\ Cardinality(DOMAIN oauth_device_flow_expires_at_millis) <= 1 /\ Cardinality(oauth_device_poll_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(oauth_browser_flow_ids) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_providers) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_redirect_uris) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_expires_at_millis) <= 2 /\ Cardinality(oauth_device_flow_ids) <= 2 /\ Cardinality(DOMAIN oauth_device_flow_providers) <= 2 /\ Cardinality(DOMAIN oauth_device_flow_expires_at_millis) <= 2 /\ Cardinality(oauth_device_poll_ids) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []oauth_flow_membership_consistent

=============================================================================
