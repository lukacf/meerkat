# PeerDirectoryReachabilityMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-comms` / `generated::peer_directory_reachability`

## State
- Phase enum: `Tracking`
- `resolved_keys`: `Set<ReachabilityKey>`
- `reachability`: `Map<ReachabilityKey, PeerReachability>`
- `last_reason`: `Map<ReachabilityKey, Option<PeerReachabilityReason>>`

## Inputs
- `ReconcileResolvedDirectory`(keys: Set<ReachabilityKey>, reachability: Map<ReachabilityKey, PeerReachability>, last_reason: Map<ReachabilityKey, Option<PeerReachabilityReason>>)
- `RecordSendSucceeded`(key: ReachabilityKey)
- `RecordSendFailed`(key: ReachabilityKey, reason: PeerReachabilityReason)

## Effects

## Invariants
- `reachability_keys_are_resolved`
- `last_reason_keys_are_resolved`

## Transitions
### `ReconcileResolvedDirectory`
- From: `Tracking`
- On: `ReconcileResolvedDirectory`(keys, reachability, last_reason)
- To: `Tracking`

### `RecordSendSucceeded`
- From: `Tracking`
- On: `RecordSendSucceeded`(key)
- Guards:
  - `key_is_resolved`
- To: `Tracking`

### `RecordSendFailed`
- From: `Tracking`
- On: `RecordSendFailed`(key, reason)
- Guards:
  - `key_is_resolved`
- To: `Tracking`

## Coverage
### Code Anchors
- `meerkat-comms/src/peer_directory_reachability_authority.rs` — canonical transient reachability authority and reconcile/send-result reducer
- `meerkat-comms/src/runtime/comms_runtime.rs` — runtime-owned resolved peer directory projection and send-result integration

### Scenarios
- `reconcile-directory` — resolved peer directory snapshot is reconciled into transient reachability state
- `send-success-marks-reachable` — successful delivery marks an already resolved peer reachable
- `send-failure-marks-unreachable` — offline or transport failures mark a resolved peer unreachable without inventing unknown peers
