---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for AuthMachine.

CONSTANTS AuthLifecyclePhaseValues, BooleanValues, CredentialUseDispositionValues, CredentialUseIntentValues, NatValues, RefreshFailureDispositionValues, SetOfStringValues, SourceCredentialPublicationDispositionValues, StringValues, InputSamples_RestoreAuthoritySnapshot_last_refresh, InputSamples_RestoreAuthoritySnapshot_refresh_attempt, InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh, InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapStringStringValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in StringValues }
MapStringU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in NatValues }
OptionAuthLifecyclePhaseValues == {None} \cup {Some(x) : x \in AuthLifecyclePhaseValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

InputSamples_RestoreAuthoritySnapshot_last_refreshCi == {None}
InputSamples_RestoreAuthoritySnapshot_last_refreshDeep == OptionU64Values
InputSamples_RestoreAuthoritySnapshot_refresh_attemptCi == {0}
InputSamples_RestoreAuthoritySnapshot_refresh_attemptDeep == 0..2
InputSamples_RestoreCredentialLifecycleSnapshot_last_refreshCi == {None}
InputSamples_RestoreCredentialLifecycleSnapshot_last_refreshDeep == OptionU64Values
InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attemptCi == {0}
InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attemptDeep == 0..2

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

VARIABLES phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining, adopted_marker_generation, released_publication_at_millis

vars == << phase, model_step_count, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining, adopted_marker_generation, released_publication_at_millis >>

Init ==
    /\ phase = "Valid"
    /\ model_step_count = 0
    /\ expires_at = None
    /\ last_refresh = None
    /\ refresh_attempt = 0
    /\ credential_present = FALSE
    /\ credential_generation = 0
    /\ credential_published_at_millis = None
    /\ credential_material_fingerprint = None
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
    /\ adopted_marker_generation = None
    /\ released_publication_at_millis = None

TerminalStutter ==
    /\ phase = "Released"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_37124ff65947ee11 == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, adopted_marker_generation, released_publication_at_millis >>
UnchangedFrame_406de655613c82f8 == UNCHANGED << last_refresh, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_9b30d4318b1032dc == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_outstanding_flow_count, release_draining, adopted_marker_generation, released_publication_at_millis >>
UnchangedFrame_abee497a1080659d == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, release_draining, adopted_marker_generation, released_publication_at_millis >>
UnchangedFrame_c11c91656dae3e2e == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, release_draining, adopted_marker_generation, released_publication_at_millis >>
UnchangedFrame_cb9efebdbe589474 == UNCHANGED << credential_generation, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_cbba22b86fca468e == UNCHANGED << credential_generation, credential_material_fingerprint >>
UnchangedFrame_e68d2f8d143b21b0 == UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining >>
UnchangedFrame_ea77929633db38c3 == UNCHANGED << oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, released_publication_at_millis >>
UnchangedFrame_f466ed6c501c70fc == UNCHANGED << expires_at, last_refresh, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining, adopted_marker_generation, released_publication_at_millis >>
UnchangedFrame_fd0e07f2cab1fc7d == UNCHANGED << expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis, credential_material_fingerprint, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count, release_draining, adopted_marker_generation, released_publication_at_millis >>

Acquire(expires_at_ts, arg_credential_published_at_millis, arg_credential_material_fingerprint) ==
    /\ phase = "Valid" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ (IF (arg_credential_material_fingerprint = None) THEN TRUE ELSE (credential_material_fingerprint # arg_credential_material_fingerprint))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = expires_at_ts
    /\ refresh_attempt' = 0
    /\ credential_present' = TRUE
    /\ credential_generation' = (credential_generation + 1)
    /\ credential_published_at_millis' = Some(arg_credential_published_at_millis)
    /\ credential_material_fingerprint' = arg_credential_material_fingerprint
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_406de655613c82f8


MarkExpiring ==
    /\ phase = "Valid"
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN TRUE ELSE ((now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)) /\ ((now_ts + refresh_window_secs) <= (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None))))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessExpiringFromValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)) /\ ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) < (now_ts + refresh_window_secs))))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessExpiredFromValid(now_ts, refresh_window_secs) ==
    /\ phase = "Valid"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) <= now_ts))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessExpiring(now_ts, refresh_window_secs) ==
    /\ phase = "Expiring"
    /\ (IF (expires_at = None) THEN TRUE ELSE (now_ts < (IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None)))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessExpiredFromExpiring(now_ts, refresh_window_secs) ==
    /\ phase = "Expiring"
    /\ (IF (expires_at = None) THEN FALSE ELSE ((IF "value" \in DOMAIN expires_at THEN expires_at["value"] ELSE None) <= now_ts))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessExpired(now_ts, refresh_window_secs) ==
    /\ phase = "Expired"
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessRefreshing(now_ts, refresh_window_secs) ==
    /\ phase = "Refreshing"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessReauthRequired(now_ts, refresh_window_secs) ==
    /\ phase = "ReauthRequired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ObserveCredentialFreshnessReleased(now_ts, refresh_window_secs) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


BeginRefreshFromValid ==
    /\ phase = "Valid"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


BeginRefreshFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


BeginRefreshFromExpired ==
    /\ phase = "Expired"
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


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
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_e68d2f8d143b21b0


ResolveRefreshFailureDispositionTransientRefreshing(http_status, oauth_error_code, local_credential_unusable) ==
    /\ phase = "Refreshing"
    /\ ((local_credential_unusable = FALSE) /\ (http_status # Some(401)) /\ (http_status # Some(403)) /\ (oauth_error_code # Some("invalid_grant")) /\ (oauth_error_code # Some("invalid_client")) /\ (oauth_error_code # Some("unauthorized_client")) /\ (oauth_error_code # Some("invalid_scope")) /\ (oauth_error_code # Some("access_denied")) /\ (oauth_error_code # Some("permission_denied")) /\ (oauth_error_code # Some("expired_token")))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveRefreshFailureDispositionPermanentRefreshing(http_status, oauth_error_code, local_credential_unusable) ==
    /\ phase = "Refreshing"
    /\ (IF (local_credential_unusable = TRUE) THEN TRUE ELSE (IF (http_status = Some(401)) THEN TRUE ELSE (IF (http_status = Some(403)) THEN TRUE ELSE (IF (oauth_error_code = Some("invalid_grant")) THEN TRUE ELSE (IF (oauth_error_code = Some("invalid_client")) THEN TRUE ELSE (IF (oauth_error_code = Some("unauthorized_client")) THEN TRUE ELSE (IF (oauth_error_code = Some("invalid_scope")) THEN TRUE ELSE (IF (oauth_error_code = Some("access_denied")) THEN TRUE ELSE (IF (oauth_error_code = Some("permission_denied")) THEN TRUE ELSE (oauth_error_code = Some("expired_token")))))))))))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


RefreshFailedTransient(disposition) ==
    /\ phase = "Refreshing"
    /\ (disposition = "Transient")
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UnchangedFrame_f466ed6c501c70fc


RefreshFailedPermanent(disposition) ==
    /\ phase = "Refreshing"
    /\ (disposition = "ReauthRequired")
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ refresh_attempt' = (refresh_attempt + 1)
    /\ UnchangedFrame_f466ed6c501c70fc


MarkReauthRequiredFromValid ==
    /\ phase = "Valid"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkReauthRequiredFromExpiring ==
    /\ phase = "Expiring"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkReauthRequiredFromExpired ==
    /\ phase = "Expired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkReauthRequiredFromRefreshing ==
    /\ phase = "Refreshing"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ClearCredentialLifecycle ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = None
    /\ last_refresh' = None
    /\ refresh_attempt' = 0
    /\ credential_present' = FALSE
    /\ credential_published_at_millis' = None
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = (IF credential_present THEN credential_published_at_millis ELSE released_publication_at_millis)
    /\ UnchangedFrame_cb9efebdbe589474


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
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = (IF credential_present THEN credential_published_at_millis ELSE released_publication_at_millis)
    /\ UnchangedFrame_cb9efebdbe589474


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
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = (IF credential_present THEN credential_published_at_millis ELSE released_publication_at_millis)
    /\ UnchangedFrame_cbba22b86fca468e


BeginReleaseDrainingOAuthFlowsValid ==
    /\ phase = "Valid"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseDrainingOAuthFlowsExpiring ==
    /\ phase = "Expiring"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseDrainingOAuthFlowsExpired ==
    /\ phase = "Expired"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseDrainingOAuthFlowsRefreshing ==
    /\ phase = "Refreshing"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseDrainingOAuthFlowsReauthRequired ==
    /\ phase = "ReauthRequired"
    /\ (oauth_outstanding_flow_count > 0)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseWithoutOAuthFlowsValid ==
    /\ phase = "Valid"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseWithoutOAuthFlowsExpiring ==
    /\ phase = "Expiring"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseWithoutOAuthFlowsExpired ==
    /\ phase = "Expired"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseWithoutOAuthFlowsRefreshing ==
    /\ phase = "Refreshing"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseWithoutOAuthFlowsReauthRequired ==
    /\ phase = "ReauthRequired"
    /\ (oauth_outstanding_flow_count = 0)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ release_draining' = TRUE
    /\ UnchangedFrame_37124ff65947ee11


BeginReleaseReleased ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


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
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = (IF credential_present THEN credential_published_at_millis ELSE released_publication_at_millis)
    /\ UnchangedFrame_cbba22b86fca468e


RestoreCredentialLifecycleSnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Valid")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ (IF (arg_adopted_marker_generation = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_adopted_marker_generation THEN arg_adopted_marker_generation["value"] ELSE None) < arg_credential_generation))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_adopted_marker_generation # None) THEN arg_adopted_marker_generation ELSE (IF (arg_credential_generation < credential_generation) THEN Some(arg_credential_generation) ELSE None))
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Expiring")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ (IF (arg_adopted_marker_generation = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_adopted_marker_generation THEN arg_adopted_marker_generation["value"] ELSE None) < arg_credential_generation))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_adopted_marker_generation # None) THEN arg_adopted_marker_generation ELSE (IF (arg_credential_generation < credential_generation) THEN Some(arg_credential_generation) ELSE None))
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Refreshing")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ (IF (arg_adopted_marker_generation = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_adopted_marker_generation THEN arg_adopted_marker_generation["value"] ELSE None) < arg_credential_generation))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_adopted_marker_generation # None) THEN arg_adopted_marker_generation ELSE (IF (arg_credential_generation < credential_generation) THEN Some(arg_credential_generation) ELSE None))
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("Expired")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ (IF (arg_adopted_marker_generation = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_adopted_marker_generation THEN arg_adopted_marker_generation["value"] ELSE None) < arg_credential_generation))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_adopted_marker_generation # None) THEN arg_adopted_marker_generation ELSE (IF (arg_credential_generation < credential_generation) THEN Some(arg_credential_generation) ELSE None))
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis) ==
    /\ phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released"
    /\ ((lifecycle_phase = Some("ReauthRequired")) /\ arg_credential_present /\ (arg_credential_published_at_millis # None))
    /\ (IF (arg_adopted_marker_generation = None) THEN TRUE ELSE ((IF "value" \in DOMAIN arg_adopted_marker_generation THEN arg_adopted_marker_generation["value"] ELSE None) < arg_credential_generation))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = arg_expires_at
    /\ last_refresh' = arg_last_refresh
    /\ refresh_attempt' = arg_refresh_attempt
    /\ credential_present' = arg_credential_present
    /\ credential_generation' = IF (arg_credential_generation > credential_generation) THEN arg_credential_generation ELSE credential_generation
    /\ credential_published_at_millis' = arg_credential_published_at_millis
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_adopted_marker_generation # None) THEN arg_adopted_marker_generation ELSE (IF (arg_credential_generation < credential_generation) THEN Some(arg_credential_generation) ELSE None))
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis) ==
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
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = arg_released_publication_at_millis
    /\ UnchangedFrame_e68d2f8d143b21b0


RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis) ==
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
    /\ credential_material_fingerprint' = None
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
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = arg_released_publication_at_millis


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
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_credential_present /\ (arg_credential_generation < credential_generation)) THEN Some(arg_credential_generation) ELSE None)
    /\ released_publication_at_millis' = (IF arg_credential_present THEN None ELSE released_publication_at_millis)
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
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_credential_present /\ (arg_credential_generation < credential_generation)) THEN Some(arg_credential_generation) ELSE None)
    /\ released_publication_at_millis' = (IF arg_credential_present THEN None ELSE released_publication_at_millis)
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
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_credential_present /\ (arg_credential_generation < credential_generation)) THEN Some(arg_credential_generation) ELSE None)
    /\ released_publication_at_millis' = (IF arg_credential_present THEN None ELSE released_publication_at_millis)
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
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_credential_present /\ (arg_credential_generation < credential_generation)) THEN Some(arg_credential_generation) ELSE None)
    /\ released_publication_at_millis' = (IF arg_credential_present THEN None ELSE released_publication_at_millis)
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
    /\ credential_material_fingerprint' = None
    /\ adopted_marker_generation' = (IF (arg_credential_present /\ (arg_credential_generation < credential_generation)) THEN Some(arg_credential_generation) ELSE None)
    /\ released_publication_at_millis' = (IF arg_credential_present THEN None ELSE released_publication_at_millis)
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
    /\ credential_material_fingerprint' = None
    /\ release_draining' = FALSE
    /\ adopted_marker_generation' = None
    /\ UnchangedFrame_ea77929633db38c3


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


RestoreOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


RestoreOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


RestoreOAuthDevicePollExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


RestoreOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


RestoreOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (release_draining = FALSE)
    /\ (flow_id \in oauth_device_flow_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


VerifyOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ConsumeOAuthBrowserFlowValid(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_abee497a1080659d


ConsumeOAuthBrowserFlowExpiring(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_abee497a1080659d


ConsumeOAuthBrowserFlowExpired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_abee497a1080659d


ConsumeOAuthBrowserFlowRefreshing(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_abee497a1080659d


ConsumeOAuthBrowserFlowReauthRequired(flow_id, provider, redirect_uri, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_browser_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_providers THEN oauth_browser_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((IF (flow_id \in DOMAIN oauth_browser_flow_redirect_uris) THEN Some((IF flow_id \in DOMAIN oauth_browser_flow_redirect_uris THEN oauth_browser_flow_redirect_uris[flow_id] ELSE "None")) ELSE None) = Some(redirect_uri))
    /\ ((flow_id \in DOMAIN oauth_browser_flow_expires_at_millis) /\ (now_millis <= (oauth_browser_flow_expires_at_millis)[flow_id]))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_browser_flow_ids' = (oauth_browser_flow_ids \ {flow_id})
    /\ oauth_browser_flow_providers' = MapRemove(oauth_browser_flow_providers, flow_id)
    /\ oauth_browser_flow_redirect_uris' = MapRemove(oauth_browser_flow_redirect_uris, flow_id)
    /\ oauth_browser_flow_expires_at_millis' = MapRemove(oauth_browser_flow_expires_at_millis, flow_id)
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


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
    /\ UnchangedFrame_abee497a1080659d


ExpireOAuthBrowserFlowAbsentValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthBrowserFlowAbsentExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthBrowserFlowAbsentExpired(flow_id) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthBrowserFlowAbsentRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthBrowserFlowAbsentReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_browser_flow_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthBrowserFlowReleased(flow_id) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


ConfirmOAuthDurableAdmissionValid(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Valid"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ConfirmOAuthDurableAdmissionExpiring(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Expiring"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ConfirmOAuthDurableAdmissionExpired(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Expired"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ConfirmOAuthDurableAdmissionRefreshing(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Refreshing"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ConfirmOAuthDurableAdmissionReauthRequired(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "ReauthRequired"
    /\ (observed_global_outstanding_flows < max_outstanding_flows)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ConfirmOAuthDurableAdmissionReleased(observed_global_outstanding_flows, max_outstanding_flows) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthDeviceFlowValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthDeviceFlowExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthDeviceFlowExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthDeviceFlowRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


VerifyOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


BeginOAuthDevicePollValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


BeginOAuthDevicePollExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


BeginOAuthDevicePollExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


BeginOAuthDevicePollRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


BeginOAuthDevicePollReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \cup {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


FinishOAuthDevicePollValid(flow_id) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


FinishOAuthDevicePollExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


FinishOAuthDevicePollExpired(flow_id) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


FinishOAuthDevicePollRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


FinishOAuthDevicePollReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_poll_ids)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ UnchangedFrame_9b30d4318b1032dc


FinishOAuthDevicePollAbsentValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


FinishOAuthDevicePollAbsentExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


FinishOAuthDevicePollAbsentExpired(flow_id) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


FinishOAuthDevicePollAbsentRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


FinishOAuthDevicePollAbsentReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_device_poll_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


FinishOAuthDevicePollReleased(flow_id) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ConsumeOAuthDeviceFlowValid(flow_id, provider, now_millis) ==
    /\ phase = "Valid"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_c11c91656dae3e2e


ConsumeOAuthDeviceFlowExpiring(flow_id, provider, now_millis) ==
    /\ phase = "Expiring"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_c11c91656dae3e2e


ConsumeOAuthDeviceFlowExpired(flow_id, provider, now_millis) ==
    /\ phase = "Expired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_c11c91656dae3e2e


ConsumeOAuthDeviceFlowRefreshing(flow_id, provider, now_millis) ==
    /\ phase = "Refreshing"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_c11c91656dae3e2e


ConsumeOAuthDeviceFlowReauthRequired(flow_id, provider, now_millis) ==
    /\ phase = "ReauthRequired"
    /\ (flow_id \in oauth_device_flow_ids)
    /\ ((IF (flow_id \in DOMAIN oauth_device_flow_providers) THEN Some((IF flow_id \in DOMAIN oauth_device_flow_providers THEN oauth_device_flow_providers[flow_id] ELSE "None")) ELSE None) = Some(provider))
    /\ ((flow_id \in DOMAIN oauth_device_flow_expires_at_millis) /\ (now_millis <= (oauth_device_flow_expires_at_millis)[flow_id]))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ oauth_device_flow_ids' = (oauth_device_flow_ids \ {flow_id})
    /\ oauth_device_flow_providers' = MapRemove(oauth_device_flow_providers, flow_id)
    /\ oauth_device_flow_expires_at_millis' = MapRemove(oauth_device_flow_expires_at_millis, flow_id)
    /\ oauth_device_poll_ids' = (oauth_device_poll_ids \ {flow_id})
    /\ oauth_outstanding_flow_count' = (oauth_outstanding_flow_count - 1)
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


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
    /\ UnchangedFrame_c11c91656dae3e2e


ExpireOAuthDeviceFlowAbsentValid(flow_id) ==
    /\ phase = "Valid"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthDeviceFlowAbsentExpiring(flow_id) ==
    /\ phase = "Expiring"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthDeviceFlowAbsentExpired(flow_id) ==
    /\ phase = "Expired"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthDeviceFlowAbsentRefreshing(flow_id) ==
    /\ phase = "Refreshing"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthDeviceFlowAbsentReauthRequired(flow_id) ==
    /\ phase = "ReauthRequired"
    /\ ((flow_id \in oauth_device_flow_ids) = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ExpireOAuthDeviceFlowReleased(flow_id) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionValidUseAuthorizedValid(intent) ==
    /\ phase = "Valid"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionValidHoldAuthorizedValid(intent) ==
    /\ phase = "Valid"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionValidBeginRefreshValid(intent) ==
    /\ phase = "Valid"
    /\ ((intent = "BeginRefresh") /\ credential_present)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionValidNoCredentialValid(intent) ==
    /\ phase = "Valid"
    /\ (credential_present = FALSE)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiringUseRefreshExpiring(intent) ==
    /\ phase = "Expiring"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiringHoldAuthorizedExpiring(intent) ==
    /\ phase = "Expiring"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiringBeginRefreshExpiring(intent) ==
    /\ phase = "Expiring"
    /\ ((intent = "BeginRefresh") /\ credential_present)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiringNoCredentialExpiring(intent) ==
    /\ phase = "Expiring"
    /\ (credential_present = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiredUseRefreshExpired(intent) ==
    /\ phase = "Expired"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiredHoldRefreshExpired(intent) ==
    /\ phase = "Expired"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiredBeginRefreshExpired(intent) ==
    /\ phase = "Expired"
    /\ ((intent = "BeginRefresh") /\ credential_present)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionExpiredNoCredentialExpired(intent) ==
    /\ phase = "Expired"
    /\ (credential_present = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionRefreshingUseRefreshRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ ((intent = "UseCredential") /\ credential_present)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionRefreshingHoldAuthorizedRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ ((intent = "HoldAuthority") /\ credential_present)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionRefreshingBeginAlreadyRefreshingRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ (intent = "BeginRefresh")
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionRefreshingNoCredentialUseOrHoldRefreshing(intent) ==
    /\ phase = "Refreshing"
    /\ ((credential_present = FALSE) /\ (IF (intent = "UseCredential") THEN TRUE ELSE (intent = "HoldAuthority")))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionReauthRequiredReauthRequired(intent) ==
    /\ phase = "ReauthRequired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveCredentialUseAdmissionReleasedReleased(intent) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionUseCachedValid(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Valid"
    /\ (credential_present /\ arg_credential_present /\ (force_refresh = FALSE))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshValidValid(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Valid"
    /\ (~((credential_present /\ arg_credential_present /\ (force_refresh = FALSE))) /\ refresh_allowed)
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Valid"
    /\ (~((credential_present /\ arg_credential_present /\ (force_refresh = FALSE))) /\ (refresh_allowed = FALSE))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshNonValidExpiring(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expiring"
    /\ refresh_allowed
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshNonValidExpired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expired"
    /\ refresh_allowed
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshNonValidRefreshing(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Refreshing"
    /\ refresh_allowed
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshNonValidReauthRequired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "ReauthRequired"
    /\ refresh_allowed
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshNonValidReleased(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Released"
    /\ refresh_allowed
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpiring(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expiring"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidExpired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Expired"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidRefreshing(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Refreshing"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReauthRequired(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "ReauthRequired"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveOAuthLoginCredentialDispositionRefreshDisallowedNonValidReleased(arg_credential_present, force_refresh, refresh_allowed) ==
    /\ phase = "Released"
    /\ (refresh_allowed = FALSE)
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationNewMaterialValid(arg_credential_material_fingerprint) ==
    /\ phase = "Valid"
    /\ (credential_material_fingerprint # Some(arg_credential_material_fingerprint))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationNewMaterialExpiring(arg_credential_material_fingerprint) ==
    /\ phase = "Expiring"
    /\ (credential_material_fingerprint # Some(arg_credential_material_fingerprint))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationNewMaterialExpired(arg_credential_material_fingerprint) ==
    /\ phase = "Expired"
    /\ (credential_material_fingerprint # Some(arg_credential_material_fingerprint))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationNewMaterialRefreshing(arg_credential_material_fingerprint) ==
    /\ phase = "Refreshing"
    /\ (credential_material_fingerprint # Some(arg_credential_material_fingerprint))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationNewMaterialReauthRequired(arg_credential_material_fingerprint) ==
    /\ phase = "ReauthRequired"
    /\ (credential_material_fingerprint # Some(arg_credential_material_fingerprint))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationNewMaterialReleased(arg_credential_material_fingerprint) ==
    /\ phase = "Released"
    /\ (credential_material_fingerprint # Some(arg_credential_material_fingerprint))
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationRenewalExpiring(arg_credential_material_fingerprint) ==
    /\ phase = "Expiring"
    /\ (credential_material_fingerprint = Some(arg_credential_material_fingerprint))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationRenewalExpired(arg_credential_material_fingerprint) ==
    /\ phase = "Expired"
    /\ (credential_material_fingerprint = Some(arg_credential_material_fingerprint))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationAlreadyPublishedValid(arg_credential_material_fingerprint) ==
    /\ phase = "Valid"
    /\ (credential_material_fingerprint = Some(arg_credential_material_fingerprint))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationAlreadyPublishedRefreshing(arg_credential_material_fingerprint) ==
    /\ phase = "Refreshing"
    /\ (credential_material_fingerprint = Some(arg_credential_material_fingerprint))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationAlreadyPublishedReauthRequired(arg_credential_material_fingerprint) ==
    /\ phase = "ReauthRequired"
    /\ (credential_material_fingerprint = Some(arg_credential_material_fingerprint))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


ResolveSourceCredentialPublicationAlreadyPublishedReleased(arg_credential_material_fingerprint) ==
    /\ phase = "Released"
    /\ (credential_material_fingerprint = Some(arg_credential_material_fingerprint))
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


AcquireFromExpiring(expires_at_ts, arg_credential_published_at_millis, arg_credential_material_fingerprint) ==
    /\ phase = "Expiring"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = expires_at_ts
    /\ refresh_attempt' = 0
    /\ credential_present' = TRUE
    /\ credential_generation' = (credential_generation + 1)
    /\ credential_published_at_millis' = Some(arg_credential_published_at_millis)
    /\ credential_material_fingerprint' = arg_credential_material_fingerprint
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_406de655613c82f8


AcquireFromExpired(expires_at_ts, arg_credential_published_at_millis, arg_credential_material_fingerprint) ==
    /\ phase = "Expired"
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ expires_at' = expires_at_ts
    /\ refresh_attempt' = 0
    /\ credential_present' = TRUE
    /\ credential_generation' = (credential_generation + 1)
    /\ credential_published_at_millis' = Some(arg_credential_published_at_millis)
    /\ credential_material_fingerprint' = arg_credential_material_fingerprint
    /\ adopted_marker_generation' = None
    /\ released_publication_at_millis' = None
    /\ UnchangedFrame_406de655613c82f8


MarkPublicationRefusedFromValid(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Valid"
    /\ ((credential_generation = refused_credential_generation) /\ (credential_material_fingerprint = refused_credential_material_fingerprint))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedFromExpiring(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Expiring"
    /\ ((credential_generation = refused_credential_generation) /\ (credential_material_fingerprint = refused_credential_material_fingerprint))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedFromExpired(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Expired"
    /\ ((credential_generation = refused_credential_generation) /\ (credential_material_fingerprint = refused_credential_material_fingerprint))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedFromRefreshing(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Refreshing"
    /\ ((credential_generation = refused_credential_generation) /\ (credential_material_fingerprint = refused_credential_material_fingerprint))
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedSupersededValid(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Valid"
    /\ (IF (credential_generation # refused_credential_generation) THEN TRUE ELSE (credential_material_fingerprint # refused_credential_material_fingerprint))
    /\ phase' = "Valid"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedSupersededExpiring(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Expiring"
    /\ (IF (credential_generation # refused_credential_generation) THEN TRUE ELSE (credential_material_fingerprint # refused_credential_material_fingerprint))
    /\ phase' = "Expiring"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedSupersededExpired(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Expired"
    /\ (IF (credential_generation # refused_credential_generation) THEN TRUE ELSE (credential_material_fingerprint # refused_credential_material_fingerprint))
    /\ phase' = "Expired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedSupersededRefreshing(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Refreshing"
    /\ (IF (credential_generation # refused_credential_generation) THEN TRUE ELSE (credential_material_fingerprint # refused_credential_material_fingerprint))
    /\ phase' = "Refreshing"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedWithdrawnReauthRequired(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "ReauthRequired"
    /\ phase' = "ReauthRequired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


MarkPublicationRefusedWithdrawnReleased(refused_credential_generation, refused_credential_material_fingerprint) ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_fd0e07f2cab1fc7d


Next ==
    \/ (phase = "Valid" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E expires_at_ts \in OptionU64Values : \E arg_credential_published_at_millis \in 0..2 : \E arg_credential_material_fingerprint \in OptionStringValues : Acquire(expires_at_ts, arg_credential_published_at_millis, arg_credential_material_fingerprint)
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
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : \E arg_adopted_marker_generation \in OptionU64Values : \E arg_released_publication_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : \E arg_adopted_marker_generation \in OptionU64Values : \E arg_released_publication_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : \E arg_adopted_marker_generation \in OptionU64Values : \E arg_released_publication_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : \E arg_adopted_marker_generation \in OptionU64Values : \E arg_released_publication_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : \E arg_adopted_marker_generation \in OptionU64Values : \E arg_released_publication_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E restored_oauth_membership_observed \in BOOLEAN : \E arg_adopted_marker_generation \in OptionU64Values : \E arg_released_publication_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, restored_oauth_membership_observed, arg_adopted_marker_generation, arg_released_publication_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in OptionAuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreCredentialLifecycleSnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreCredentialLifecycleSnapshot_refresh_attempt : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : \E arg_adopted_marker_generation \in OptionU64Values : \E arg_released_publication_at_millis \in OptionU64Values : RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis, FALSE, arg_adopted_marker_generation, arg_released_publication_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreAuthoritySnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreAuthoritySnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotValid(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreAuthoritySnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreAuthoritySnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotExpiring(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreAuthoritySnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreAuthoritySnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotRefreshing(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreAuthoritySnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreAuthoritySnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotExpired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, TRUE, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreAuthoritySnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreAuthoritySnapshot_refresh_attempt : \E arg_credential_present \in BOOLEAN : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotReauthRequired(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, arg_credential_present, arg_credential_generation, arg_credential_published_at_millis)
    \/ (phase = "Valid" \/ phase = "Expiring" \/ phase = "Expired" \/ phase = "Refreshing" \/ phase = "ReauthRequired" \/ phase = "Released") /\ \E lifecycle_phase \in AuthLifecyclePhaseValues : \E arg_expires_at \in OptionU64Values : \E arg_last_refresh \in InputSamples_RestoreAuthoritySnapshot_last_refresh : \E arg_refresh_attempt \in InputSamples_RestoreAuthoritySnapshot_refresh_attempt : \E arg_credential_generation \in 0..2 : \E arg_credential_published_at_millis \in OptionU64Values : RestoreAuthoritySnapshotReleased(lifecycle_phase, arg_expires_at, arg_last_refresh, arg_refresh_attempt, FALSE, arg_credential_generation, arg_credential_published_at_millis)
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
    \/ (phase = "Valid") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationNewMaterialValid(arg_credential_material_fingerprint)
    \/ (phase = "Expiring") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationNewMaterialExpiring(arg_credential_material_fingerprint)
    \/ (phase = "Expired") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationNewMaterialExpired(arg_credential_material_fingerprint)
    \/ (phase = "Refreshing") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationNewMaterialRefreshing(arg_credential_material_fingerprint)
    \/ (phase = "ReauthRequired") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationNewMaterialReauthRequired(arg_credential_material_fingerprint)
    \/ (phase = "Released") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationNewMaterialReleased(arg_credential_material_fingerprint)
    \/ (phase = "Expiring") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationRenewalExpiring(arg_credential_material_fingerprint)
    \/ (phase = "Expired") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationRenewalExpired(arg_credential_material_fingerprint)
    \/ (phase = "Valid") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationAlreadyPublishedValid(arg_credential_material_fingerprint)
    \/ (phase = "Refreshing") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationAlreadyPublishedRefreshing(arg_credential_material_fingerprint)
    \/ (phase = "ReauthRequired") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationAlreadyPublishedReauthRequired(arg_credential_material_fingerprint)
    \/ (phase = "Released") /\ \E arg_credential_material_fingerprint \in StringValues : ResolveSourceCredentialPublicationAlreadyPublishedReleased(arg_credential_material_fingerprint)
    \/ (phase = "Expiring") /\ \E expires_at_ts \in OptionU64Values : \E arg_credential_published_at_millis \in 0..2 : \E arg_credential_material_fingerprint \in OptionStringValues : AcquireFromExpiring(expires_at_ts, arg_credential_published_at_millis, arg_credential_material_fingerprint)
    \/ (phase = "Expired") /\ \E expires_at_ts \in OptionU64Values : \E arg_credential_published_at_millis \in 0..2 : \E arg_credential_material_fingerprint \in OptionStringValues : AcquireFromExpired(expires_at_ts, arg_credential_published_at_millis, arg_credential_material_fingerprint)
    \/ (phase = "Valid") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedFromValid(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Expiring") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedFromExpiring(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Expired") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedFromExpired(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Refreshing") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedFromRefreshing(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Valid") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedSupersededValid(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Expiring") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedSupersededExpiring(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Expired") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedSupersededExpired(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Refreshing") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedSupersededRefreshing(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "ReauthRequired") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedWithdrawnReauthRequired(refused_credential_generation, refused_credential_material_fingerprint)
    \/ (phase = "Released") /\ \E refused_credential_generation \in 0..2 : \E refused_credential_material_fingerprint \in OptionStringValues : MarkPublicationRefusedWithdrawnReleased(refused_credential_generation, refused_credential_material_fingerprint)
    \/ TerminalStutter

oauth_flow_membership_consistent == ((DOMAIN oauth_browser_flow_providers = oauth_browser_flow_ids) /\ (DOMAIN oauth_browser_flow_redirect_uris = oauth_browser_flow_ids) /\ (DOMAIN oauth_browser_flow_expires_at_millis = oauth_browser_flow_ids) /\ (DOMAIN oauth_device_flow_providers = oauth_device_flow_ids) /\ (DOMAIN oauth_device_flow_expires_at_millis = oauth_device_flow_ids) /\ (\A flow_id \in oauth_device_poll_ids : (flow_id \in oauth_device_flow_ids)) /\ (oauth_outstanding_flow_count = (Cardinality(oauth_browser_flow_ids) + Cardinality(oauth_device_flow_ids))))
released_oauth_membership_drained == (IF (phase # "Released") THEN TRUE ELSE (oauth_outstanding_flow_count = 0))
released_not_release_draining == (IF (phase # "Released") THEN TRUE ELSE (release_draining = FALSE))
adopted_marker_generation_below_credential_generation == (IF (adopted_marker_generation = None) THEN TRUE ELSE (credential_present /\ ((IF "value" \in DOMAIN adopted_marker_generation THEN adopted_marker_generation["value"] ELSE None) < credential_generation)))
released_publication_only_without_credential == (IF credential_present THEN (released_publication_at_millis = None) ELSE TRUE)

CiStateConstraint == /\ model_step_count <= 3 /\ Cardinality(oauth_browser_flow_ids) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_providers) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_redirect_uris) <= 1 /\ Cardinality(DOMAIN oauth_browser_flow_expires_at_millis) <= 1 /\ Cardinality(oauth_device_flow_ids) <= 1 /\ Cardinality(DOMAIN oauth_device_flow_providers) <= 1 /\ Cardinality(DOMAIN oauth_device_flow_expires_at_millis) <= 1 /\ Cardinality(oauth_device_poll_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(oauth_browser_flow_ids) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_providers) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_redirect_uris) <= 2 /\ Cardinality(DOMAIN oauth_browser_flow_expires_at_millis) <= 2 /\ Cardinality(oauth_device_flow_ids) <= 2 /\ Cardinality(DOMAIN oauth_device_flow_providers) <= 2 /\ Cardinality(DOMAIN oauth_device_flow_expires_at_millis) <= 2 /\ Cardinality(oauth_device_poll_ids) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []oauth_flow_membership_consistent
THEOREM Spec => []released_oauth_membership_drained
THEOREM Spec => []released_not_release_draining
THEOREM Spec => []adopted_marker_generation_below_credential_generation
THEOREM Spec => []released_publication_only_without_credential

=============================================================================
