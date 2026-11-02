# MobStoreRestoreLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobStoreRestoreLifecycleMachine`

### Code Anchors
- `mob_store_restore_lifecycle` (machine `MobStoreRestoreLifecycleMachine`): `crates/meerkat-mob/src/machines/mob_store_restore_lifecycle.rs` — MobStoreRestoreLifecycleMachine owns one quarantined persisted mob store: its class, the single re-attempt in flight, and whether a re-attempt released the store or left it quarantined under the class it met

### Scenarios
- `mob_store_quarantine_classification` — mob_store_restore_lifecycle_machine.rs::a_pass_classifies_a_store_once quarantines an unclassified store under its class and refuses a second classification in every later phase
- `mob_store_single_flight_retry` — mob_store_restore_lifecycle_machine.rs::one_re_attempt_is_in_flight_at_a_time admits one re-attempt of a quarantined store, refuses a second request while it runs without waiting, and refuses a request for an unclassified or released store
- `mob_store_retry_eligibility` — mob_store_restore_lifecycle_machine.rs::an_on_demand_re_attempt_is_admitted_only_for_a_store_that_could_not_serve admits an on-demand re-attempt only for a store quarantined as store_unavailable, refuses it unchanged for store_unreadable and resume_refused, and admits an explicit re-attempt for every class
- `mob_store_retry_outcomes` — mob_store_restore_lifecycle_machine.rs::a_re_attempt_releases_the_store_or_keeps_it_quarantined releases the store on a managed, held or settled outcome, keeps it quarantined under the class a failed attempt met, counts every ended attempt, and refuses an outcome with no attempt in flight

### Transitions
- `QuarantineUnclassified`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_quarantine_classification`
- `QuarantineQuarantined`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_quarantine_classification`
- `QuarantineRetrying`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_quarantine_classification`
- `QuarantineReleased`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_quarantine_classification`
- `RequestRetryUnclassified`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_single_flight_retry`
- `RequestRetryQuarantined`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_single_flight_retry`, `mob_store_retry_eligibility`
- `RequestRetryQuarantinedNotRetryable`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_eligibility`
- `RequestRetryRetrying`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_single_flight_retry`
- `RequestRetryReleased`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_single_flight_retry`
- `ResolveRetryFailed`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`
- `ResolveRetryRestored`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`
- `ResolveRetryUnclassified`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`
- `ResolveRetryQuarantined`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`
- `ResolveRetryReleased`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`

### Effects
- `StoreQuarantined`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_quarantine_classification`
- `RetryAdmitted`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_single_flight_retry`, `mob_store_retry_eligibility`
- `StoreReleased`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`
- `StoreStillQuarantined`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`
- `RequestRefused`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_quarantine_classification`, `mob_store_single_flight_retry`, `mob_store_retry_eligibility`, `mob_store_retry_outcomes`

### Invariants
- `unclassified_record_has_no_attempts`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_quarantine_classification`
- `released_record_was_attempted`
  - anchors: `mob_store_restore_lifecycle`
  - scenarios: `mob_store_retry_outcomes`


<!-- GENERATED_COVERAGE_END -->
