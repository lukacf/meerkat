# AuthMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::auth_machine`

## State
- Phase enum: `Valid | Expiring | Refreshing | ReauthRequired | Released`
- `expires_at`: `Option<u64>`
- `last_refresh`: `Option<u64>`
- `refresh_attempt`: `u64`

## Inputs
- `Acquire`(expires_at_ts: Option<u64>)
- `MarkExpiring`
- `BeginRefresh`
- `CompleteRefresh`(new_expires_at: Option<u64>, now_ts: u64)
- `RefreshFailedTransient`
- `RefreshFailedPermanent`
- `MarkReauthRequired`
- `Release`

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

### `Release`
- From: `Valid`, `Expiring`, `Refreshing`, `ReauthRequired`, `Released`
- On: `Release`()
- Emits: `EmitLifecycleEvent`
- To: `Released`

## Coverage
### Code Anchors
- `meerkat-runtime/src/handles/auth_lease.rs` — per-binding AuthMachine registry; AuthLeaseHandle trait impl drives DSL transitions through it

### Scenarios
- `acquire_expire_refresh_complete` — lease transitions through valid, expiring, refreshing, and back to valid on successful refresh
