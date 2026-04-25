# AuthMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `AuthMachine`

### Code Anchors
- `auth_lease_handle`: `meerkat-runtime/src/handles/auth_lease.rs` — per-binding AuthMachine registry; AuthLeaseHandle trait impl drives acquire, expiring, refresh, reauth, release, lifecycle event, and wake loop DSL transitions through it

### Scenarios
- `acquire_expire_refresh_complete` — lease transitions through valid, expiring, refreshing, and back to valid on successful refresh
- `reauth_release_and_publication` — reauth required from valid/expiring/refreshing, release lease, emit lifecycle event, and wake refresh loop publication

### Transitions
- `Acquire`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `MarkExpiring`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`, `reauth_release_and_publication`
- `BeginRefreshFromValid`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`
- `BeginRefreshFromExpiring`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`
- `CompleteRefresh`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`
- `RefreshFailedTransient`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`, `reauth_release_and_publication`
- `RefreshFailedPermanent`
  - anchors: `auth_lease_handle`
  - scenarios: `acquire_expire_refresh_complete`, `reauth_release_and_publication`
- `MarkReauthRequiredFromValid`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`
- `MarkReauthRequiredFromExpiring`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`
- `MarkReauthRequiredFromRefreshing`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`
- `Release`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`

### Effects
- `EmitLifecycleEvent`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`
- `WakeRefreshLoop`
  - anchors: `auth_lease_handle`
  - scenarios: `reauth_release_and_publication`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
