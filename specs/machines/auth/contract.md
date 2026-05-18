# AuthMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::auth_machine`

## State
- Phase enum: `Valid | Expiring | Refreshing | ReauthRequired | Released`
- `expires_at`: `Option<u64>`
- `last_refresh`: `Option<u64>`
- `refresh_attempt`: `u64`
- `credential_present`: `Bool`
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
- `Acquire`(expires_at_ts: Option<u64>)
- `MarkExpiring`
- `BeginRefresh`
- `CompleteRefresh`(new_expires_at: Option<u64>, now_ts: u64)
- `RefreshFailedTransient`
- `RefreshFailedPermanent`
- `MarkReauthRequired`
- `ClearCredentialLifecycle`
- `Release`
- `RestoreAuthoritySnapshot`(lifecycle_phase: AuthLifecyclePhase, expires_at: Option<u64>, last_refresh: Option<u64>, refresh_attempt: u64, credential_present: Bool, oauth_browser_flow_ids: Set<String>, oauth_browser_flow_providers: Map<String, String>, oauth_browser_flow_redirect_uris: Map<String, String>, oauth_browser_flow_expires_at_millis: Map<String, u64>, oauth_device_flow_ids: Set<String>, oauth_device_flow_providers: Map<String, String>, oauth_device_flow_expires_at_millis: Map<String, u64>, oauth_device_poll_ids: Set<String>, oauth_outstanding_flow_count: u64)
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
- `EmitLifecycleEvent`(new_state: AuthLifecyclePhase)
- `WakeRefreshLoop`

## Invariants

## Transitions
### `Acquire`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `Acquire`(expires_at_ts)
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `MarkExpiring`
- From: `Valid`
- On: `MarkExpiring`()
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

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

### `CompleteRefresh`
- From: `Refreshing`
- On: `CompleteRefresh`(new_expires_at, now_ts)
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `RefreshFailedTransient`
- From: `Refreshing`
- On: `RefreshFailedTransient`()
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `RefreshFailedPermanent`
- From: `Refreshing`
- On: `RefreshFailedPermanent`()
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

### `MarkReauthRequiredFromRefreshing`
- From: `Refreshing`
- On: `MarkReauthRequired`()
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `ClearCredentialLifecycle`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `ClearCredentialLifecycle`()
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `Release`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `Release`()
- Emits: `EmitLifecycleEvent`
- To: `Released`

### `RestoreAuthoritySnapshotValid`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Valid`

### `RestoreAuthoritySnapshotExpiring`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Expiring`

### `RestoreAuthoritySnapshotRefreshing`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Refreshing`

### `RestoreAuthoritySnapshotReauthRequired`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `ReauthRequired`

### `RestoreAuthoritySnapshotReleased`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `RestoreAuthoritySnapshot`(lifecycle_phase, expires_at, last_refresh, refresh_attempt, credential_present, oauth_browser_flow_ids, oauth_browser_flow_providers, oauth_browser_flow_redirect_uris, oauth_browser_flow_expires_at_millis, oauth_device_flow_ids, oauth_device_flow_providers, oauth_device_flow_expires_at_millis, oauth_device_poll_ids, oauth_outstanding_flow_count)
- Guards:
  - ``
- Emits: `EmitLifecycleEvent`
- To: `Released`

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
- `meerkat-runtime/src/handles/auth_lease.rs` — per-binding AuthMachine registry; AuthLeaseHandle trait impl drives acquire, expiring, refresh, reauth, release, lifecycle event, and wake loop DSL transitions through it
- `meerkat-runtime/src/handles/oauth_flow.rs` — per-binding AuthMachine-owned OAuth browser and device flow lifecycle authority for admit, verify, begin poll, finish poll, consume, expire, valid, expiring, refreshing, and reauth required phases

### Scenarios
- `acquire_expire_refresh_complete` — lease transitions through valid, expiring, refreshing, and back to valid on successful refresh
- `reauth_release_and_publication` — reauth required from valid/expiring/refreshing, release lease, emit lifecycle event, and wake refresh loop publication
- `oauth_browser_flow_lifecycle` — OAuth browser flow admit, verify, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority
- `oauth_device_flow_lifecycle` — OAuth device flow admit, verify, begin poll, finish poll, consume, and expire operations stay under the per-binding AuthMachine lifecycle authority
