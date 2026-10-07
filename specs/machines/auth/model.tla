---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for AuthMachine.

CONSTANTS AuthLifecyclePhaseValues, BooleanValues, CredentialUseDispositionValues, CredentialUseIntentValues, NatValues, RefreshFailureDispositionValues, SetOfStringValues, StringValues

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

VARIABLES phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining

vars == << phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>

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
    /\ release_draining = FALSE

TerminalStutter ==
    /\ phase = "Released"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_131bb96ef518525a == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_17e882200453b6ed == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, release_draining >>
UnchangedFrame_406de655613c82f8 == UNCHANGED << last_refresh, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_4c7cae2d3aa4ff6d == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>
UnchangedFrame_58d7ab2993337708 == UNCHANGED << credential_generation, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_70b7c14ac09c236c == UNCHANGED << expires_at, last_refresh, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_84b4777df2a321b4 == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, release_draining >>
UnchangedFrame_c1e698ecd35aa39a == UNCHANGED << credential_generation >>
UnchangedFrame_e68d2f8d143b21b0 == UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_e9fda9e62fb02e30 == UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count >>
UnchangedFrame_f87ebf4401320955 == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>

Acquire(expires_at_ts, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = expires_at_ts
    /\ refresh_attempt' = 0
    /\ credential_present' = TRUE
    /\ credential_generation' = (credential_generation + 1)
    /\ credential_published_at_millis' = Some(arg_credential_published_at_millis)
    /\ UnchangedFrame_406de655613c82f8


MarkExpiring ==
    /\ phase = "Valid"
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN TRUE ELSE ((now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)) /\ ((now_ts + refresh_window_secs) <= (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None))))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessExpiringFromValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)) /\ ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) < (now_ts + refresh_window_secs))))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessExpiredFromValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) <= now_ts))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessExpiring(now_ts, refresh_window_secs) ==
    /\ phase = "Expiring"
    /\ (IF (expires_at = None) THEN TRUE ELSE (now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessExpiredFromExpiring(now_ts, refresh_window_secs) ==
    /\ phase = "Expiring"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) <= now_ts))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessExpired(now_ts, refresh_window_secs) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessRefreshing(now_ts, refresh_window_secs) ==
    /\ phase = "Refreshing"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessReauthRequired(now_ts, refresh_window_secs) ==
    /\ phase = "ReauthRequired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ObserveCredentialFreshnessReleased(now_ts, refresh_window_secs) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


BeginRefreshFromValid ==
    /\ phase = "Valid"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


BeginRefreshFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


BeginRefreshFromExpired ==
    /\ phase = "Expired"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


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
    /\ UnchangedFrame_e68d2f8d143b21b0


ResolveRefreshFailureDispositionTransientRefreshing(http_status, oauth_error_code, local_credential_unusable) ==
    /\ phase = "Refreshing"
    /\ ((local_credential_unusable = FALSE) /\ (http_status # Some(401)) /\ (http_status # Some(403)) /\ (oauth_error_code # Some("invalid_grant")) /\ (oauth_error_code # Some("invalid_client")) /\ (oauth_error_code # Some("unauthorized_client")) /\ (oauth_error_code # Some("invalid_scope")) /\ (oauth_error_code # Some("access_denied")) /\ (oauth_error_code # Some("permission_denied")) /\ (oauth_error_code # Some("expired_token")))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveRefreshFailureDispositionPermanentRefreshing(http_status, oauth_error_code, local_credential_unusable) ==
    /\ phase = "Refreshing"
    /\ (IF (local_credential_unusable = TRUE) THEN TRUE ELSE (IF (http_status = Some(401)) THEN TRUE ELSE (IF (http_status = Some(403)) THEN TRUE ELSE (IF (oauth_error_code = Some("invalid_grant")) THEN TRUE ELSE (IF (oauth_error_code = Some("invalid_client")) THEN TRUE ELSE (IF (oauth_error_code = Some("unauthorized_client")) THEN TRUE ELSE (IF (oauth_error_code = Some("invalid_scope")) THEN TRUE ELSE (IF (oauth_error_code = Some("access_denied")) THEN TRUE ELSE (IF (oauth_error_code = Some("permission_denied")) THEN TRUE ELSE (oauth_error_code = Some("expired_token")))))))))))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


RefreshFailedTransient(disposition) ==
    /\ phase = "Refreshing"
    /\ (disposition = "Transient")
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UnchangedFrame_70b7c14ac09c236c


RefreshFailedPermanent(disposition) ==
    /\ phase = "Refreshing"
    /\ (disposition = "ReauthRequired")
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UnchangedFrame_70b7c14ac09c236c


MarkReauthRequiredFromValid ==
    /\ phase = "Valid"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


MarkReauthRequiredFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


MarkReauthRequiredFromExpired ==
    /\ phase = "Expired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


MarkReauthRequiredFromRefreshing ==
    /\ phase = "Refreshing"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ClearCredentialLifecycle ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_published_at_millis' = None
    /\ UnchangedFrame_58d7ab2993337708


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
    /\ UnchangedFrame_58d7ab2993337708


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
    /\ release_draining' = FALSE
    /\ UnchangedFrame_c1e698ecd35aa39a


BeginReleaseDrainingOAuthFlowsValid ==
    /\ phase = "Valid"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseDrainingOAuthFlowsExpiring ==
    /\ phase = "Expiring"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseDrainingOAuthFlowsExpired ==
    /\ phase = "Expired"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseDrainingOAuthFlowsRefreshing ==
    /\ phase = "Refreshing"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseDrainingOAuthFlowsReauthRequired ==
    /\ phase = "ReauthRequired"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseWithoutOAuthFlowsValid ==
    /\ phase = "Valid"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseWithoutOAuthFlowsExpiring ==
    /\ phase = "Expiring"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseWithoutOAuthFlowsExpired ==
    /\ phase = "Expired"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseWithoutOAuthFlowsRefreshing ==
    /\ phase = "Refreshing"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseWithoutOAuthFlowsReauthRequired ==
    /\ phase = "ReauthRequired"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_4c7cae2d3aa4ff6d


BeginReleaseReleased ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


Release ==
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
    /\ release_draining' = FALSE
    /\ UnchangedFrame_c1e698ecd35aa39a


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
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ (IF (arg_credential_present = FALSE) THEN TRUE ELSE (IF (lifecycle_phase = None) THEN TRUE ELSE (lifecycle_phase = Some("Released"))))
    /\ (IF (oauth_outstanding_flow_count > 0) THEN TRUE ELSE restored_oauth_membership_observed)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = None
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ (IF (arg_credential_present = FALSE) THEN TRUE ELSE (IF (lifecycle_phase = None) THEN TRUE ELSE (lifecycle_phase = Some("Released"))))
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
    /\ release_draining' = FALSE


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
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreAuthoritySnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = "ReauthRequired") /\ (IF (arg_credential_present = FALSE) THEN TRUE ELSE (arg_credential_published_at_millis # None)))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ UnchangedFrame_e68d2f8d143b21b0


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
    /\ release_draining' = FALSE
    /\ UnchangedFrame_e9fda9e62fb02e30


RestoreOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Valid"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


RestoreOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Expiring"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


RestoreOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Expired"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


RestoreOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "Refreshing"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


RestoreOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis) ==
    /\ phase = "ReauthRequired"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


RestoreOAuthDeviceFlowValid(flow_id, provider, expires_at_millis) ==
    /\ phase = "Valid"
    /\ (release_draining = FALSE)
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UnchangedFrame_84b4777df2a321b4


RestoreOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis) ==
    /\ phase = "Expiring"
    /\ (release_draining = FALSE)
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UnchangedFrame_84b4777df2a321b4


RestoreOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis) ==
    /\ phase = "Expired"
    /\ (release_draining = FALSE)
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UnchangedFrame_84b4777df2a321b4


RestoreOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis) ==
    /\ phase = "Refreshing"
    /\ (release_draining = FALSE)
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UnchangedFrame_84b4777df2a321b4


RestoreOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis) ==
    /\ phase = "ReauthRequired"
    /\ (release_draining = FALSE)
    /\ (provider # None)
    /\ (expires_at_millis # None)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \cup {flow_id})
    /\ oauth_device_flow_providers' = MapSet(oauth_device_flow_providers, flow_id, (IF "value" \in DOMAIN provider THEN provider["value"] ELSE None))
    /\ oauth_device_flow_expires_at_millis' = MapSet(oauth_device_flow_expires_at_millis, flow_id, (IF "value" \in DOMAIN expires_at_millis THEN expires_at_millis["value"] ELSE None))
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = IF ((flow_id \in oauth_device_flow_ids) = FALSE) THEN (oauth_outstanding_flow_count + 1) ELSE oauth_outstanding_flow_count
    /\ UnchangedFrame_84b4777df2a321b4


RestoreOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


RestoreOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


RestoreOAuthDevicePollExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


RestoreOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


RestoreOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


AdmitOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Valid"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


AdmitOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expiring"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


AdmitOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expired"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


AdmitOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Refreshing"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


AdmitOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "ReauthRequired"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


VerifyOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_expires_at_millis THEN oauth_browser_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


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
    /\ UnchangedFrame_17e882200453b6ed


ExpireOAuthBrowserFlowAbsentValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthBrowserFlowAbsentExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthBrowserFlowAbsentExpired(flow_id) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthBrowserFlowAbsentRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthBrowserFlowAbsentReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthBrowserFlowReleased(flow_id) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


AdmitOAuthDeviceFlowValid(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Valid"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_84b4777df2a321b4


AdmitOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expiring"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_84b4777df2a321b4


AdmitOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Expired"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_84b4777df2a321b4


AdmitOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "Refreshing"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_84b4777df2a321b4


AdmitOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows) ==
    /\ phase = "ReauthRequired"
    /\ (release_draining = FALSE)
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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


ConfirmOAuthDurableAdmissionValid(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Valid"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ConfirmOAuthDurableAdmissionExpiring(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Expiring"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ConfirmOAuthDurableAdmissionExpired(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Expired"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ConfirmOAuthDurableAdmissionRefreshing(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Refreshing"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ConfirmOAuthDurableAdmissionReauthRequired(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "ReauthRequired"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ConfirmOAuthDurableAdmissionReleased(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthDeviceFlowValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthDeviceFlowExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthDeviceFlowExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthDeviceFlowRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


VerifyOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


BeginOAuthDevicePollValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


BeginOAuthDevicePollExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


BeginOAuthDevicePollExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


BeginOAuthDevicePollRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


BeginOAuthDevicePollReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ (now_millis <= (IF "value" \in DOMAIN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None) THEN (IF (flow_id \in DOMAIN oauth_device_flow_expires_at_millis) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_expires_at_millis THEN oauth_device_flow_expires_at_millis[flow_id] ELSE 0)) ELSE None)["value"] ELSE None))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


FinishOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


FinishOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


FinishOAuthDevicePollExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


FinishOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


FinishOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_131bb96ef518525a


FinishOAuthDevicePollAbsentValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


FinishOAuthDevicePollAbsentExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


FinishOAuthDevicePollAbsentExpired(flow_id) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


FinishOAuthDevicePollAbsentRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


FinishOAuthDevicePollAbsentReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


FinishOAuthDevicePollReleased(flow_id) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


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
    /\ UnchangedFrame_84b4777df2a321b4


ExpireOAuthDeviceFlowAbsentValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthDeviceFlowAbsentExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthDeviceFlowAbsentExpired(flow_id) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthDeviceFlowAbsentRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthDeviceFlowAbsentReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ExpireOAuthDeviceFlowReleased(flow_id) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionValidUseAuthorizedValid(intent) ==
    /\ phase = "Valid"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionValidHoldAuthorizedValid(intent) ==
    /\ phase = "Valid"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionValidBeginRefreshValid(intent) ==
    /\ phase = "Valid"
    /\ ((intent = "BeginRefresh") /\ credential_present)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionValidNoCredentialValid(intent) ==
    /\ phase = "Valid"
    /\ (credential_present = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiringUseRefreshExpiring(intent) ==
    /\ phase = "Expiring"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring(intent) ==
    /\ phase = "Expiring"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring(intent) ==
    /\ phase = "Expiring"
    /\ ((intent = "BeginRefresh") /\ credential_present)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiringNoCredentialExpiring(intent) ==
    /\ phase = "Expiring"
    /\ (credential_present = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiredUseRefreshExpired(intent) ==
    /\ phase = "Expired"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiredHoldRefreshExpired(intent) ==
    /\ phase = "Expired"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiredBeginRefreshExpired(intent) ==
    /\ phase = "Expired"
    /\ ((intent = "BeginRefresh") /\ credential_present)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionExpiredNoCredentialExpired(intent) ==
    /\ phase = "Expired"
    /\ (credential_present = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ (intent = "BeginRefresh")
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ ((credential_present = FALSE) /\ (IF (intent = "UseCredential") THEN TRUE ELSE (intent = "HoldAuthority")))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionReauthRequiredReauthRequired(intent) ==
    /\ phase = "ReauthRequired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveCredentialUseAdmissionReleasedReleased(intent) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionUseCachedValid(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Valid"
    /\ (credential_present /\ arg_credential_present /\ (force_refresh = FALSE))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshValidValid(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Valid"
    /\ (~((credential_present /\ arg_credential_present /\ (force_refresh = FALSE))) /\ refresh_allowed)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Valid"
    /\ (~((credential_present /\ arg_credential_present /\ (force_refresh = FALSE))) /\ (refresh_allowed = FALSE))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expiring"
    /\ refresh_allowed
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expired"
    /\ refresh_allowed
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Refreshing"
    /\ refresh_allowed
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "ReauthRequired"
    /\ refresh_allowed
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Released"
    /\ refresh_allowed
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expiring"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expired"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Refreshing"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "ReauthRequired"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Released"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_f87ebf4401320955


Next ==
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E expires_at_ts \in OptionU64Values : \E arg_credential_published_at_millis \in 0..2 : Acquire(expires_at_ts, arg_credential_published_at_millis)
    \/ MarkExpiring
    \/ (phase = "Valid") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessValid(now_ts, refresh_window_secs)
    \/ (phase = "Valid") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiringFromValid(now_ts, refresh_window_secs)
    \/ (phase = "Valid") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiredFromValid(now_ts, refresh_window_secs)
    \/ (phase = "Expiring") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiring(now_ts, refresh_window_secs)
    \/ (phase = "Expiring") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpiredFromExpiring(now_ts, refresh_window_secs)
    \/ (phase = "Expired") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessExpired(now_ts, refresh_window_secs)
    \/ (phase = "Refreshing") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessRefreshing(now_ts, refresh_window_secs)
    \/ (phase = "ReauthRequired") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessReauthRequired(now_ts, refresh_window_secs)
    \/ (phase = "Released") /\ \E now_ts \in 0..2 : \E refresh_window_secs \in 0..2 : ObserveCredentialFreshnessReleased(now_ts, refresh_window_secs)
    \/ BeginRefreshFromValid
    \/ BeginRefreshFromExpiring
    \/ BeginRefreshFromExpired
    \/ (phase = "Refreshing") /\ \E new_expires_at \in OptionU64Values : \E now_ts \in 0..2 : \E arg_credential_published_at_millis \in 0..2 : CompleteRefresh(new_expires_at, now_ts, arg_credential_published_at_millis)
    \/ (phase = "Refreshing") /\ \E http_status \in OptionU64Values : \E oauth_error_code \in OptionStringValues : ResolveRefreshFailureDispositionTransientRefreshing(http_status, oauth_error_code, FALSE)
    \/ (phase = "Refreshing") /\ \E http_status \in OptionU64Values : \E oauth_error_code \in OptionStringValues : \E local_credential_unusable \in BOOLEAN : ResolveRefreshFailureDispositionPermanentRefreshing(http_status, oauth_error_code, local_credential_unusable)
    \/ (phase = "Refreshing") /\ \E disposition \in RefreshFailureDispositionValues : RefreshFailedTransient(disposition)
    \/ (phase = "Refreshing") /\ \E disposition \in RefreshFailureDispositionValues : RefreshFailedPermanent(disposition)
    \/ MarkReauthRequiredFromValid
    \/ MarkReauthRequiredFromExpiring
    \/ MarkReauthRequiredFromExpired
    \/ MarkReauthRequiredFromRefreshing
    \/ ClearCredentialLifecycle
    \/ ReleaseCredentialLifecycleWithOAuth
    \/ ReleaseCredentialLifecycleWithoutOAuth
    \/ BeginReleaseDrainingOAuthFlowsValid
    \/ BeginReleaseDrainingOAuthFlowsExpiring
    \/ BeginReleaseDrainingOAuthFlowsExpired
    \/ BeginReleaseDrainingOAuthFlowsRefreshing
    \/ BeginReleaseDrainingOAuthFlowsReauthRequired
    \/ BeginReleaseWithoutOAuthFlowsValid
    \/ BeginReleaseWithoutOAuthFlowsExpiring
    \/ BeginReleaseWithoutOAuthFlowsExpired
    \/ BeginReleaseWithoutOAuthFlowsRefreshing
    \/ BeginReleaseWithoutOAuthFlowsReauthRequired
    \/ BeginReleaseReleased
    \/ Release
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, FALSE)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in OptionU64Values : \E arg_refresh_attempt \in 0..2 : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotReleased(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, FALSE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E redirect_uri \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowValid(flow_id, provider, expires_at_millis)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in OptionStringValues : \E expires_at_millis \in OptionU64Values : RestoreOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : RestoreOAuthDevicePollValid(flow_id)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : RestoreOAuthDevicePollExpiring(flow_id)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : RestoreOAuthDevicePollExpired(flow_id)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : RestoreOAuthDevicePollRefreshing(flow_id)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : RestoreOAuthDevicePollReauthRequired(flow_id)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowValid(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Released") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : ReopenReleasedForOAuthBrowserFlowAdmission(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : VerifyOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E redirect_uri \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowValid(flow_id)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowExpiring(flow_id)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowExpired(flow_id)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowRefreshing(flow_id)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowReauthRequired(flow_id)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowAbsentValid(flow_id)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowAbsentExpiring(flow_id)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowAbsentExpired(flow_id)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowAbsentRefreshing(flow_id)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowAbsentReauthRequired(flow_id)
    \/ (phase = "Released") /\ \E flow_id \in StringValues : ExpireOAuthBrowserFlowReleased(flow_id)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowValid(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowExpiring(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowExpired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowRefreshing(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : AdmitOAuthDeviceFlowReauthRequired(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Released") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E expires_at_millis \in 0..2 : \E max_outstanding_flows \in 0..2 : \E observed_global_outstanding_flows \in 0..2 : ReopenReleasedForOAuthDeviceFlowAdmission(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
    \/ (phase = "Valid") /\ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionValid(observed_global_outstanding_flows, max_outstanding_flows)
    \/ (phase = "Expiring") /\ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionExpiring(observed_global_outstanding_flows, max_outstanding_flows)
    \/ (phase = "Expired") /\ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionExpired(observed_global_outstanding_flows, max_outstanding_flows)
    \/ (phase = "Refreshing") /\ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionRefreshing(observed_global_outstanding_flows, max_outstanding_flows)
    \/ (phase = "ReauthRequired") /\ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionReauthRequired(observed_global_outstanding_flows, max_outstanding_flows)
    \/ (phase = "Released") /\ \E observed_global_outstanding_flows \in 0..2 : \E max_outstanding_flows \in 0..2 : ConfirmOAuthDurableAdmissionReleased(observed_global_outstanding_flows, max_outstanding_flows)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowValid(flow_id, provider, now_millis)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowExpiring(flow_id, provider, now_millis)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowExpired(flow_id, provider, now_millis)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowRefreshing(flow_id, provider, now_millis)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : VerifyOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollValid(flow_id, provider, now_millis)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollExpiring(flow_id, provider, now_millis)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollExpired(flow_id, provider, now_millis)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollRefreshing(flow_id, provider, now_millis)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : BeginOAuthDevicePollReauthRequired(flow_id, provider, now_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : FinishOAuthDevicePollValid(flow_id)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : FinishOAuthDevicePollExpiring(flow_id)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : FinishOAuthDevicePollExpired(flow_id)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : FinishOAuthDevicePollRefreshing(flow_id)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : FinishOAuthDevicePollReauthRequired(flow_id)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : FinishOAuthDevicePollAbsentValid(flow_id)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : FinishOAuthDevicePollAbsentExpiring(flow_id)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : FinishOAuthDevicePollAbsentExpired(flow_id)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : FinishOAuthDevicePollAbsentRefreshing(flow_id)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : FinishOAuthDevicePollAbsentReauthRequired(flow_id)
    \/ (phase = "Released") /\ \E flow_id \in StringValues : FinishOAuthDevicePollReleased(flow_id)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowValid(flow_id, provider, now_millis)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowExpiring(flow_id, provider, now_millis)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowExpired(flow_id, provider, now_millis)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowRefreshing(flow_id, provider, now_millis)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : \E provider \in StringValues : \E now_millis \in 0..2 : ConsumeOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowValid(flow_id)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowExpiring(flow_id)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowExpired(flow_id)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowRefreshing(flow_id)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowReauthRequired(flow_id)
    \/ (phase = "Valid") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowAbsentValid(flow_id)
    \/ (phase = "Expiring") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowAbsentExpiring(flow_id)
    \/ (phase = "Expired") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowAbsentExpired(flow_id)
    \/ (phase = "Refreshing") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowAbsentRefreshing(flow_id)
    \/ (phase = "ReauthRequired") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowAbsentReauthRequired(flow_id)
    \/ (phase = "Released") /\ \E flow_id \in StringValues : ExpireOAuthDeviceFlowReleased(flow_id)
    \/ (phase = "Valid") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionValidUseAuthorizedValid(intent)
    \/ (phase = "Valid") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionValidHoldAuthorizedValid(intent)
    \/ (phase = "Valid") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionValidBeginRefreshValid(intent)
    \/ (phase = "Valid") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionValidNoCredentialValid(intent)
    \/ (phase = "Expiring") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiringUseRefreshExpiring(intent)
    \/ (phase = "Expiring") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring(intent)
    \/ (phase = "Expiring") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring(intent)
    \/ (phase = "Expiring") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiringNoCredentialExpiring(intent)
    \/ (phase = "Expired") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiredUseRefreshExpired(intent)
    \/ (phase = "Expired") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiredHoldRefreshExpired(intent)
    \/ (phase = "Expired") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiredBeginRefreshExpired(intent)
    \/ (phase = "Expired") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionExpiredNoCredentialExpired(intent)
    \/ (phase = "Refreshing") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing(intent)
    \/ (phase = "Refreshing") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing(intent)
    \/ (phase = "Refreshing") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing(intent)
    \/ (phase = "Refreshing") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing(intent)
    \/ (phase = "ReauthRequired") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionReauthRequiredReauthRequired(intent)
    \/ (phase = "Released") /\ \E intent \in CredentialUseIntentValues : ResolveCredentialUseAdmissionReleasedReleased(intent)
    \/ (phase = "Valid") /\ \E refresh_allowed \in BOOLEAN : ResolveOAuthLoginCredentialDispositionUseCachedValid(TRUE, FALSE, refresh_allowed)
    \/ (phase = "Valid") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshValidValid(arg_credential_present, force_refresh, TRUE)
    \/ (phase = "Valid") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid(arg_credential_present, force_refresh, FALSE)
    \/ (phase = "Expiring") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring(arg_credential_present, force_refresh, TRUE)
    \/ (phase = "Expired") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired(arg_credential_present, force_refresh, TRUE)
    \/ (phase = "Refreshing") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing(arg_credential_present, force_refresh, TRUE)
    \/ (phase = "ReauthRequired") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired(arg_credential_present, force_refresh, TRUE)
    \/ (phase = "Released") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased(arg_credential_present, force_refresh, TRUE)
    \/ (phase = "Expiring") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring(arg_credential_present, force_refresh, FALSE)
    \/ (phase = "Expired") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired(arg_credential_present, force_refresh, FALSE)
    \/ (phase = "Refreshing") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing(arg_credential_present, force_refresh, FALSE)
    \/ (phase = "ReauthRequired") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired(arg_credential_present, force_refresh, FALSE)
    \/ (phase = "Released") /\ \E arg_credential_present \in BOOLEAN : \E force_refresh \in BOOLEAN : ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased(arg_credential_present, force_refresh, FALSE)
    \/ TerminalStutter

oauth_flow_membership_consistent == ((DOMAIN oauth_browser_flow_providers = oauth_browser_flow_ids) /\ (DOMAIN oauth_browser_flow_redirect_uris = oauth_browser_flow_ids) /\ (DOMAIN oauth_browser_flow_expires_at_millis = oauth_browser_flow_ids) /\ (DOMAIN oauth_device_flow_providers = oauth_device_flow_ids) /\ (DOMAIN oauth_device_flow_expires_at_millis = oauth_device_flow_ids) /\ (\A flow_id \in oauth_device_poll_ids : (flow_id \in oauth_device_flow_ids)) /\ (oauth_outstanding_flow_count = (Cardinality(oauth_browser_flow_ids) + Cardinality(oauth_device_flow_ids))))
released_oauth_membership_drained == (IF (phase # "Released") THEN TRUE ELSE (oauth_outstanding_flow_count = 0))
released_not_release_draining == (IF (phase # "Released") THEN TRUE ELSE (release_draining = FALSE))

CiStateConstraint == /\ model_step_count <= 3 /\ Cardinality(oauth_browser_flow_ids) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_providers) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_redirect_uris) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_expires_at_millis) <= 1 /\ Cardinality(oauth_device_flow_ids) <= 1 /\ Cardinality(DOMAIN oauth_device_flow_providers) <= 1 /\ Cardinality(DOMAIN oauth_device_flow_expires_at_millis) <= 1 /\ Cardinality(oauth_device_poll_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(oauth_browser_flow_ids) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_providers) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_redirect_uris) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_expires_at_millis) <= 2 /\ Cardinality(oauth_device_flow_ids) <= 2 /\ Cardinality(DOMAIN oauth_device_flow_providers) <= 2 /\ Cardinality(DOMAIN oauth_device_flow_expires_at_millis) <= 2 /\ Cardinality(oauth_device_poll_ids) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []oauth_flow_membership_consistent
THEOREM Spec => []released_oauth_membership_drained
THEOREM Spec => []released_not_release_draining

=============================================================================
