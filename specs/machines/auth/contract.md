# AuthMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::auth_machine`

## State
- Phase enum: `Valid | Expiring | Expired | Refreshing | ReauthRequired | Released`
- `expires_at`: `Option<u64>`
- `last_refresh`: `Option<u64>`
- `refresh_attempt`: `u64`
- `credential_present`: `Bool`
- `credential_generation`: `u64`
- `credential_published_at_millis`: `Option<u64>`
- `oauth_browser_flow_ids`: `Set<String>`
- `oauth_browser_flow_providers`: `Map<String, String>`
- `oauth_browser_flow_redirect_uris`: `Map<String, String>`
- `oauth_browser_flow_expires_at_millis`: `Map<String, u64>`
- `oauth_device_flow_ids`: `Set<String>`
- `oauth_device_flow_providers`: `Map<String, String>`
- `oauth_device_flow_expires_at_millis`: `Map<String, u64>`
- `oauth_device_poll_ids`: `Set<String>`
- `oauth_outstanding_flow_count`: `u64`

## Inputs
- `Acquire`(expires_at_ts: Option<u64>, credential_published_at_millis: u64)
- `MarkExpiring`
- `ObserveCredentialFreshness`(now_ts: u64, refresh_window_secs: u64)
- `BeginRefresh`
- `CompleteRefresh`(new_expires_at: Option<u64>, now_ts: u64, credential_published_at_millis: u64)
- `RefreshFailed`(http_status: Option<u64>, oauth_error_code: Option<String>, local_credential_unusable: Bool)
- `MarkReauthRequired`
- `ClearCredentialLifecycle`
- `Release`
- `RestoreAuthoritySnapshot`(lifecycle_phase: AuthLifecyclePhase, expires_at: Option<u64>, last_refresh: Option<u64>, refresh_attempt: u64, credential_present: Bool, credential_generation: u64, credential_published_at_millis: Option<u64>)
- `RestoreOAuthBrowserFlow`(flow_id: String, provider: Option<String>, redirect_uri: Option<String>, expires_at_millis: Option<u64>)
- `RestoreOAuthDeviceFlow`(flow_id: String, provider: Option<String>, expires_at_millis: Option<u64>)
- `RestoreOAuthDevicePoll`(flow_id: String)
- `AdmitOAuthBrowserFlow`(flow_id: String, provider: String, redirect_uri: String, expires_at_millis: u64, max_outstanding_flows: u64, observed_global_outstanding_flows: u64)
- `VerifyOAuthBrowserFlow`(flow_id: String, provider: String, redirect_uri: String, now_millis: u64)
- `ConsumeOAuthBrowserFlow`(flow_id: String, provider: String, redirect_uri: String, now_millis: u64)
- `ExpireOAuthBrowserFlow`(flow_id: String)
- `AdmitOAuthDeviceFlow`(flow_id: String, provider: String, expires_at_millis: u64, max_outstanding_flows: u64, observed_global_outstanding_flows: u64)
- `ConfirmOAuthDurableAdmission`(observed_global_outstanding_flows: u64, max_outstanding_flows: u64)
- `VerifyOAuthDeviceFlow`(flow_id: String, provider: String, now_millis: u64)
- `BeginOAuthDevicePoll`(flow_id: String, provider: String, now_millis: u64)
- `FinishOAuthDevicePoll`(flow_id: String)
- `ConsumeOAuthDeviceFlow`(flow_id: String, provider: String, now_millis: u64)
- `ExpireOAuthDeviceFlow`(flow_id: String)

## Signals

## Effects
- `EmitLifecycleEvent`(new_state: AuthLifecyclePhase, expires_at: Option<u64>, credential_generation: u64, credential_published_at_millis: Option<u64>)
- `WakeRefreshLoop`

## Invariants
- `oauth_flow_membership_consistent`

## Transitions
### `Acquire`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `Acquire`(expires_at_ts, credential_published_at_millis)
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `MarkExpiring`
- From: `Valid`
- On: `MarkExpiring`()
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `ObserveCredentialFreshnessValid`
- From: `Valid`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Guards:
  - `credential_not_in_refresh_window`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `ObserveCredentialFreshnessExpiringFromValid`
- From: `Valid`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Guards:
  - `credential_in_refresh_window`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `ObserveCredentialFreshnessExpiredFromValid`
- From: `Valid`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Guards:
  - `credential_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `ObserveCredentialFreshnessExpiring`
- From: `Expiring`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Guards:
  - `credential_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `ObserveCredentialFreshnessExpiredFromExpiring`
- From: `Expiring`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Guards:
  - `credential_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `ObserveCredentialFreshnessExpired`
- From: `Expired`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `ObserveCredentialFreshnessRefreshing`
- From: `Refreshing`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `ObserveCredentialFreshnessReauthRequired`
- From: `ReauthRequired`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ObserveCredentialFreshnessReleased`
- From: `Released`
- On: `ObserveCredentialFreshness`(now_ts, refresh_window_secs)
- Emits: `EmitLifecycleEvent`
- To: `Released`

### `BeginRefreshFromValid`
- From: `Valid`
- On: `BeginRefresh`()
- Emits: `EmitLifecycleEvent`, `WakeRefreshLoop`
- To: `Refreshing`

### `BeginRefreshFromExpiring`
- From: `Expiring`
- On: `BeginRefresh`()
- Emits: `EmitLifecycleEvent`, `WakeRefreshLoop`
- To: `Refreshing`

### `BeginRefreshFromExpired`
- From: `Expired`
- On: `BeginRefresh`()
- Emits: `EmitLifecycleEvent`, `WakeRefreshLoop`
- To: `Refreshing`

### `CompleteRefresh`
- From: `Refreshing`
- On: `CompleteRefresh`(new_expires_at, now_ts, credential_published_at_millis)
- Guards:
  - `new_credential_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `RefreshFailedTransient`
- From: `Refreshing`
- On: `RefreshFailed`(http_status, oauth_error_code, local_credential_unusable)
- Guards:
  - `refresh_failure_observation_transient`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `RefreshFailedPermanent`
- From: `Refreshing`
- On: `RefreshFailed`(http_status, oauth_error_code, local_credential_unusable)
- Guards:
  - `refresh_failure_observation_permanent`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `MarkReauthRequiredFromValid`
- From: `Valid`
- On: `MarkReauthRequired`()
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `MarkReauthRequiredFromExpiring`
- From: `Expiring`
- On: `MarkReauthRequired`()
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `MarkReauthRequiredFromExpired`
- From: `Expired`
- On: `MarkReauthRequired`()
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `MarkReauthRequiredFromRefreshing`
- From: `Refreshing`
- On: `MarkReauthRequired`()
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ClearCredentialLifecycle`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `ClearCredentialLifecycle`()
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `Release`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `Release`()
- Emits: `EmitLifecycleEvent`
- To: `Released`

### `RestoreAuthoritySnapshotValid`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `RestoreAuthoritySnapshotExpiring`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `RestoreAuthoritySnapshotRefreshing`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `RestoreAuthoritySnapshotExpired`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `RestoreAuthoritySnapshotReauthRequired`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `RestoreAuthoritySnapshotReleased`
- From: `Valid`, `Expiring`, `Expired`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, credential_generation, credential_published_at_millis)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Released`

### `RestoreOAuthBrowserFlowValid`
- From: `Valid`
- On: `RestoreOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis)
- Guards:
  - `browser_restore_has_provider`
  - `browser_restore_has_redirect_uri`
  - `browser_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `RestoreOAuthBrowserFlowExpiring`
- From: `Expiring`
- On: `RestoreOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis)
- Guards:
  - `browser_restore_has_provider`
  - `browser_restore_has_redirect_uri`
  - `browser_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `RestoreOAuthBrowserFlowExpired`
- From: `Expired`
- On: `RestoreOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis)
- Guards:
  - `browser_restore_has_provider`
  - `browser_restore_has_redirect_uri`
  - `browser_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `RestoreOAuthBrowserFlowRefreshing`
- From: `Refreshing`
- On: `RestoreOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis)
- Guards:
  - `browser_restore_has_provider`
  - `browser_restore_has_redirect_uri`
  - `browser_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `RestoreOAuthBrowserFlowReauthRequired`
- From: `ReauthRequired`
- On: `RestoreOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis)
- Guards:
  - `browser_restore_has_provider`
  - `browser_restore_has_redirect_uri`
  - `browser_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `RestoreOAuthDeviceFlowValid`
- From: `Valid`
- On: `RestoreOAuthDeviceFlow`(flow_id, provider, expires_at_millis)
- Guards:
  - `device_restore_has_provider`
  - `device_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `RestoreOAuthDeviceFlowExpiring`
- From: `Expiring`
- On: `RestoreOAuthDeviceFlow`(flow_id, provider, expires_at_millis)
- Guards:
  - `device_restore_has_provider`
  - `device_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `RestoreOAuthDeviceFlowExpired`
- From: `Expired`
- On: `RestoreOAuthDeviceFlow`(flow_id, provider, expires_at_millis)
- Guards:
  - `device_restore_has_provider`
  - `device_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `RestoreOAuthDeviceFlowRefreshing`
- From: `Refreshing`
- On: `RestoreOAuthDeviceFlow`(flow_id, provider, expires_at_millis)
- Guards:
  - `device_restore_has_provider`
  - `device_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `RestoreOAuthDeviceFlowReauthRequired`
- From: `ReauthRequired`
- On: `RestoreOAuthDeviceFlow`(flow_id, provider, expires_at_millis)
- Guards:
  - `device_restore_has_provider`
  - `device_restore_has_expiry`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `RestoreOAuthDevicePollValid`
- From: `Valid`
- On: `RestoreOAuthDevicePoll`(flow_id)
- Guards:
  - `device_flow_present_for_poll_restore`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `RestoreOAuthDevicePollExpiring`
- From: `Expiring`
- On: `RestoreOAuthDevicePoll`(flow_id)
- Guards:
  - `device_flow_present_for_poll_restore`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `RestoreOAuthDevicePollExpired`
- From: `Expired`
- On: `RestoreOAuthDevicePoll`(flow_id)
- Guards:
  - `device_flow_present_for_poll_restore`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `RestoreOAuthDevicePollRefreshing`
- From: `Refreshing`
- On: `RestoreOAuthDevicePoll`(flow_id)
- Guards:
  - `device_flow_present_for_poll_restore`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `RestoreOAuthDevicePollReauthRequired`
- From: `ReauthRequired`
- On: `RestoreOAuthDevicePoll`(flow_id)
- Guards:
  - `device_flow_present_for_poll_restore`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `AdmitOAuthBrowserFlowValid`
- From: `Valid`
- On: `AdmitOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `browser_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `AdmitOAuthBrowserFlowExpiring`
- From: `Expiring`
- On: `AdmitOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `browser_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `AdmitOAuthBrowserFlowExpired`
- From: `Expired`
- On: `AdmitOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `browser_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `AdmitOAuthBrowserFlowRefreshing`
- From: `Refreshing`
- On: `AdmitOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `browser_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `AdmitOAuthBrowserFlowReauthRequired`
- From: `ReauthRequired`
- On: `AdmitOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `browser_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ReopenReleasedForOAuthBrowserFlowAdmission`
- From: `Released`
- On: `AdmitOAuthBrowserFlow`(flow_id, provider, redirect_uri, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `released_without_credential`
  - `released_without_oauth_membership`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `VerifyOAuthBrowserFlowValid`
- From: `Valid`
- On: `VerifyOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `VerifyOAuthBrowserFlowExpiring`
- From: `Expiring`
- On: `VerifyOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `VerifyOAuthBrowserFlowExpired`
- From: `Expired`
- On: `VerifyOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `VerifyOAuthBrowserFlowRefreshing`
- From: `Refreshing`
- On: `VerifyOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `VerifyOAuthBrowserFlowReauthRequired`
- From: `ReauthRequired`
- On: `VerifyOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ConsumeOAuthBrowserFlowValid`
- From: `Valid`
- On: `ConsumeOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `ConsumeOAuthBrowserFlowExpiring`
- From: `Expiring`
- On: `ConsumeOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `ConsumeOAuthBrowserFlowExpired`
- From: `Expired`
- On: `ConsumeOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `ConsumeOAuthBrowserFlowRefreshing`
- From: `Refreshing`
- On: `ConsumeOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `ConsumeOAuthBrowserFlowReauthRequired`
- From: `ReauthRequired`
- On: `ConsumeOAuthBrowserFlow`(flow_id, provider, redirect_uri, now_millis)
- Guards:
  - `browser_flow_present`
  - `browser_flow_provider_matches`
  - `browser_flow_redirect_uri_matches`
  - `browser_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ExpireOAuthBrowserFlowValid`
- From: `Valid`
- On: `ExpireOAuthBrowserFlow`(flow_id)
- Guards:
  - `browser_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `ExpireOAuthBrowserFlowExpiring`
- From: `Expiring`
- On: `ExpireOAuthBrowserFlow`(flow_id)
- Guards:
  - `browser_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `ExpireOAuthBrowserFlowExpired`
- From: `Expired`
- On: `ExpireOAuthBrowserFlow`(flow_id)
- Guards:
  - `browser_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `ExpireOAuthBrowserFlowRefreshing`
- From: `Refreshing`
- On: `ExpireOAuthBrowserFlow`(flow_id)
- Guards:
  - `browser_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `ExpireOAuthBrowserFlowReauthRequired`
- From: `ReauthRequired`
- On: `ExpireOAuthBrowserFlow`(flow_id)
- Guards:
  - `browser_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `AdmitOAuthDeviceFlowValid`
- From: `Valid`
- On: `AdmitOAuthDeviceFlow`(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `device_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `AdmitOAuthDeviceFlowExpiring`
- From: `Expiring`
- On: `AdmitOAuthDeviceFlow`(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `device_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `AdmitOAuthDeviceFlowExpired`
- From: `Expired`
- On: `AdmitOAuthDeviceFlow`(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `device_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `AdmitOAuthDeviceFlowRefreshing`
- From: `Refreshing`
- On: `AdmitOAuthDeviceFlow`(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `device_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `AdmitOAuthDeviceFlowReauthRequired`
- From: `ReauthRequired`
- On: `AdmitOAuthDeviceFlow`(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `device_flow_absent`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ReopenReleasedForOAuthDeviceFlowAdmission`
- From: `Released`
- On: `AdmitOAuthDeviceFlow`(flow_id, provider, expires_at_millis, max_outstanding_flows, observed_global_outstanding_flows)
- Guards:
  - `released_without_credential`
  - `released_without_oauth_membership`
  - `oauth_capacity_available`
  - `oauth_global_capacity_available`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ConfirmOAuthDurableAdmissionValid`
- From: `Valid`
- On: `ConfirmOAuthDurableAdmission`(observed_global_outstanding_flows, max_outstanding_flows)
- Guards:
  - `oauth_global_capacity_available`
- To: `Valid`

### `ConfirmOAuthDurableAdmissionExpiring`
- From: `Expiring`
- On: `ConfirmOAuthDurableAdmission`(observed_global_outstanding_flows, max_outstanding_flows)
- Guards:
  - `oauth_global_capacity_available`
- To: `Expiring`

### `ConfirmOAuthDurableAdmissionExpired`
- From: `Expired`
- On: `ConfirmOAuthDurableAdmission`(observed_global_outstanding_flows, max_outstanding_flows)
- Guards:
  - `oauth_global_capacity_available`
- To: `Expired`

### `ConfirmOAuthDurableAdmissionRefreshing`
- From: `Refreshing`
- On: `ConfirmOAuthDurableAdmission`(observed_global_outstanding_flows, max_outstanding_flows)
- Guards:
  - `oauth_global_capacity_available`
- To: `Refreshing`

### `ConfirmOAuthDurableAdmissionReauthRequired`
- From: `ReauthRequired`
- On: `ConfirmOAuthDurableAdmission`(observed_global_outstanding_flows, max_outstanding_flows)
- Guards:
  - `oauth_global_capacity_available`
- To: `ReauthRequired`

### `VerifyOAuthDeviceFlowValid`
- From: `Valid`
- On: `VerifyOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `VerifyOAuthDeviceFlowExpiring`
- From: `Expiring`
- On: `VerifyOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `VerifyOAuthDeviceFlowExpired`
- From: `Expired`
- On: `VerifyOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `VerifyOAuthDeviceFlowRefreshing`
- From: `Refreshing`
- On: `VerifyOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `VerifyOAuthDeviceFlowReauthRequired`
- From: `ReauthRequired`
- On: `VerifyOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `BeginOAuthDevicePollValid`
- From: `Valid`
- On: `BeginOAuthDevicePoll`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
  - `device_poll_absent`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `BeginOAuthDevicePollExpiring`
- From: `Expiring`
- On: `BeginOAuthDevicePoll`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
  - `device_poll_absent`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `BeginOAuthDevicePollExpired`
- From: `Expired`
- On: `BeginOAuthDevicePoll`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
  - `device_poll_absent`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `BeginOAuthDevicePollRefreshing`
- From: `Refreshing`
- On: `BeginOAuthDevicePoll`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
  - `device_poll_absent`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `BeginOAuthDevicePollReauthRequired`
- From: `ReauthRequired`
- On: `BeginOAuthDevicePoll`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
  - `device_poll_absent`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `FinishOAuthDevicePollValid`
- From: `Valid`
- On: `FinishOAuthDevicePoll`(flow_id)
- Guards:
  - `device_poll_present`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `FinishOAuthDevicePollExpiring`
- From: `Expiring`
- On: `FinishOAuthDevicePoll`(flow_id)
- Guards:
  - `device_poll_present`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `FinishOAuthDevicePollExpired`
- From: `Expired`
- On: `FinishOAuthDevicePoll`(flow_id)
- Guards:
  - `device_poll_present`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `FinishOAuthDevicePollRefreshing`
- From: `Refreshing`
- On: `FinishOAuthDevicePoll`(flow_id)
- Guards:
  - `device_poll_present`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `FinishOAuthDevicePollReauthRequired`
- From: `ReauthRequired`
- On: `FinishOAuthDevicePoll`(flow_id)
- Guards:
  - `device_poll_present`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ConsumeOAuthDeviceFlowValid`
- From: `Valid`
- On: `ConsumeOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `ConsumeOAuthDeviceFlowExpiring`
- From: `Expiring`
- On: `ConsumeOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `ConsumeOAuthDeviceFlowExpired`
- From: `Expired`
- On: `ConsumeOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `ConsumeOAuthDeviceFlowRefreshing`
- From: `Refreshing`
- On: `ConsumeOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `ConsumeOAuthDeviceFlowReauthRequired`
- From: `ReauthRequired`
- On: `ConsumeOAuthDeviceFlow`(flow_id, provider, now_millis)
- Guards:
  - `device_flow_present`
  - `device_flow_provider_matches`
  - `device_flow_not_expired`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ExpireOAuthDeviceFlowValid`
- From: `Valid`
- On: `ExpireOAuthDeviceFlow`(flow_id)
- Guards:
  - `device_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `ExpireOAuthDeviceFlowExpiring`
- From: `Expiring`
- On: `ExpireOAuthDeviceFlow`(flow_id)
- Guards:
  - `device_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `ExpireOAuthDeviceFlowExpired`
- From: `Expired`
- On: `ExpireOAuthDeviceFlow`(flow_id)
- Guards:
  - `device_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Expired`

### `ExpireOAuthDeviceFlowRefreshing`
- From: `Refreshing`
- On: `ExpireOAuthDeviceFlow`(flow_id)
- Guards:
  - `device_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `ExpireOAuthDeviceFlowReauthRequired`
- From: `ReauthRequired`
- On: `ExpireOAuthDeviceFlow`(flow_id)
- Guards:
  - `device_flow_present`
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

## Coverage
### Code Anchors
- `meerkat-runtime/src/handles/auth_lease.rs` — per-binding AuthMachine registry; AuthLeaseHandle trait impl drives acquire, observe credential freshness, expiring, expired, refresh, reauth, release, lifecycle event, and wake loop DSL transitions through it
- `meerkat-runtime/src/handles/oauth_flow.rs` — per-binding AuthMachine-owned OAuth browser and device flow lifecycle authority for admit, verify, begin poll, finish poll, consume, expire, valid, expiring, expired, refreshing, and reauth required phases

### Scenarios
- `acquire_expire_refresh_complete` — lease transitions through valid, expiring, expired, refreshing, and back to valid on successful refresh
- `reauth_release_and_publication` — reauth required from valid/expiring/expired/refreshing, observe credential freshness for released state, release lease, emit lifecycle event, and wake refresh loop publication
- `oauth_browser_flow_lifecycle` — OAuth browser flow admit, verify, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority
- `oauth_device_flow_lifecycle` — OAuth device flow admit, verify, begin poll, finish poll, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority
