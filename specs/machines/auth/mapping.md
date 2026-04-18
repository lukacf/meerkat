# AuthMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `AuthMachine`

### Code Anchors
- `auth_lease_handle`: `meerkat-runtime/src/handles/auth_lease.rs` — per-binding AuthMachine registry; AuthLeaseHandle trait impl drives DSL transitions through it

### Scenarios
- `acquire_expire_refresh_complete` — lease transitions through valid, expiring, refreshing, and back to valid on successful refresh

### Transitions
- `Acquire`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `MarkExpiring`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `BeginRefreshFromValid`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `BeginRefreshFromExpiring`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `CompleteRefresh`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `RefreshFailedTransient`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `RefreshFailedPermanent`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `MarkReauthRequiredFromValid`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `MarkReauthRequiredFromExpiring`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `MarkReauthRequiredFromRefreshing`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `Release`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`

### Effects
- `EmitLifecycleEvent`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `WakeRefreshLoop`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
