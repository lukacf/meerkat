# MobStoreRestoreLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::mob_store_restore_lifecycle`

## State
- Phase enum: `Unclassified | Quarantined | Retrying | Released`
- `class`: `MobQuarantineClass`
- `attempts`: `u64`

## Inputs
- `Quarantine`(class: MobQuarantineClass)
- `RequestRetry`(trigger: MobStoreRetryTrigger)
- `ResolveRetry`(outcome: MobStoreRestoreOutcome, class: MobQuarantineClass)

## Signals

## Effects
- `StoreQuarantined`(class: MobQuarantineClass)
- `RetryAdmitted`(attempts: u64)
- `StoreReleased`(outcome: MobStoreRestoreOutcome, attempts: u64)
- `StoreStillQuarantined`(class: MobQuarantineClass, attempts: u64)
- `RequestRefused`(reason: MobStoreRestoreRefusal)

## Invariants
- `unclassified_record_has_no_attempts`
- `released_record_was_attempted`

## Transitions
### `QuarantineUnclassified`
- From: `Unclassified`
- On: `Quarantine`(class)
- Emits: `StoreQuarantined`
- To: `Quarantined`

### `QuarantineQuarantined`
- From: `Quarantined`
- On: `Quarantine`(class)
- Emits: `RequestRefused`
- To: `Quarantined`

### `QuarantineRetrying`
- From: `Retrying`
- On: `Quarantine`(class)
- Emits: `RequestRefused`
- To: `Retrying`

### `QuarantineReleased`
- From: `Released`
- On: `Quarantine`(class)
- Emits: `RequestRefused`
- To: `Released`

### `RequestRetryUnclassified`
- From: `Unclassified`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Unclassified`

### `RequestRetryQuarantined`
- From: `Quarantined`
- On: `RequestRetry`(trigger)
- Guards:
  - `retryable`
- Emits: `RetryAdmitted`
- To: `Retrying`

### `RequestRetryQuarantinedNotRetryable`
- From: `Quarantined`
- On: `RequestRetry`(trigger)
- Guards:
  - `on_demand`
  - `needs_repair`
- Emits: `RequestRefused`
- To: `Quarantined`

### `RequestRetryRetrying`
- From: `Retrying`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Retrying`

### `RequestRetryReleased`
- From: `Released`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Released`

### `ResolveRetryFailed`
- From: `Retrying`
- On: `ResolveRetry`(outcome, class)
- Guards:
  - `failed`
- Emits: `StoreStillQuarantined`
- To: `Quarantined`

### `ResolveRetryRestored`
- From: `Retrying`
- On: `ResolveRetry`(outcome, class)
- Guards:
  - `not_failed`
- Emits: `StoreReleased`
- To: `Released`

### `ResolveRetryUnclassified`
- From: `Unclassified`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Unclassified`

### `ResolveRetryQuarantined`
- From: `Quarantined`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Quarantined`

### `ResolveRetryReleased`
- From: `Released`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Released`

## Coverage
### Code Anchors
- `mob_store_restore_lifecycle` (machine `MobStoreRestoreLifecycleMachine`): `crates/meerkat-mob/src/machines/mob_store_restore_lifecycle.rs` — MobStoreRestoreLifecycleMachine owns one quarantined persisted mob store: its class, the single re-attempt in flight, and whether a re-attempt released the store or left it quarantined under the class it met

### Scenarios
- `mob_store_quarantine_classification` — mob_store_restore_lifecycle_machine.rs::a_pass_classifies_a_store_once quarantines an unclassified store under its class and refuses a second classification in every later phase
- `mob_store_single_flight_retry` — mob_store_restore_lifecycle_machine.rs::one_re_attempt_is_in_flight_at_a_time admits one re-attempt of a quarantined store, refuses a second request while it runs without waiting, and refuses a request for an unclassified or released store
- `mob_store_retry_eligibility` — mob_store_restore_lifecycle_machine.rs::an_on_demand_re_attempt_is_admitted_only_for_a_store_that_could_not_serve admits an on-demand re-attempt only for a store quarantined as store_unavailable, refuses it unchanged for store_unreadable and resume_refused, and admits an explicit re-attempt for every class
- `mob_store_retry_outcomes` — mob_store_restore_lifecycle_machine.rs::a_re_attempt_releases_the_store_or_keeps_it_quarantined releases the store on a managed, held or settled outcome, keeps it quarantined under the class a failed attempt met, counts every ended attempt, and refuses an outcome with no attempt in flight
