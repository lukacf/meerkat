# MobStoreRestoreLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `7`
- Rust owner: `self` / `catalog::dsl::mob_store_restore_lifecycle`

## State
- Phase enum: `Dormant | Loading | Managed | Unloading | Held | Quarantined | Retrying | Gone | Creating`
- `class`: `MobQuarantineClass`
- `attempts`: `u64`
- `origin`: `MobStoreOrigin`
- `served`: `Bool`
- `awaits_launch`: `Bool`
- `vacant`: `Bool`
- `stop`: `MobStoreStop`
- `known`: `Bool`

## Inputs
- `Census`
- `Register`(holder_ended: Bool)
- `Declare`(vacant: Bool)
- `EndLaunchWait`
- `RequestLoad`(trigger: MobStoreLoadTrigger)
- `ResolveLoad`(outcome: MobStoreRestoreOutcome, class: MobQuarantineClass)
- `RequestRetry`(trigger: MobStoreRetryTrigger)
- `ResolveRetry`(outcome: MobStoreRestoreOutcome, class: MobQuarantineClass)
- `RequestUnload`
- `ResolveUnload`(outcome: MobStoreUnloadOutcome, stop: MobStoreStop)
- `ReleaseHeld`(outcome: MobStoreRestoreOutcome)
- `Destroy`
- `RequestCreate`(launch: Bool)
- `ResolveCreate`(outcome: MobStoreCreateOutcome, in_memory: Bool)
- `ReadStop`(stopped: Bool)

## Signals

## Effects
- `StoreListed`
- `StoreRegistered`(origin: MobStoreOrigin)
- `LaunchDeclared`
- `LaunchWaitEnded`
- `LoadAdmitted`
- `StoreLoaded`(outcome: MobStoreRestoreOutcome)
- `StoreQuarantined`(class: MobQuarantineClass)
- `RetryAdmitted`(attempts: u64)
- `StoreReleased`(outcome: MobStoreRestoreOutcome, attempts: u64)
- `StoreStillQuarantined`(class: MobQuarantineClass, attempts: u64)
- `UnloadAdmitted`
- `StoreUnloaded`
- `UnloadDeclined`
- `HoldReleased`(outcome: MobStoreRestoreOutcome)
- `StoreGone`
- `CreateAdmitted`
- `CreateReleased`
- `StopRead`(stopped: Bool)
- `RequestRefused`(reason: MobStoreRestoreRefusal)

## Invariants
- `only_a_quarantine_holds_a_class`
- `only_a_persisted_store_waits_unloaded`
- `only_a_declared_store_awaits_its_launch`
- `a_store_that_is_gone_awaits_no_launch`
- `a_store_whose_unload_is_admitted_awaits_no_launch`
- `a_declared_store_is_never_gone`
- `only_a_declared_store_that_is_not_loaded_is_vacant`
- `only_a_store_that_is_not_loaded_knows_its_mobs_stop`
- `a_mob_is_handed_out_exactly_while_managed`
- `a_record_that_stands_for_nothing_is_as_it_was_filed`

## Transitions
### `CensusDormant`
- From: `Dormant`
- On: `Census`()
- Emits: `StoreListed`
- To: `Dormant`

### `CensusQuarantined`
- From: `Quarantined`
- On: `Census`()
- Emits: `StoreListed`
- To: `Dormant`

### `CensusLoading`
- From: `Loading`
- On: `Census`()
- Emits: `RequestRefused`
- To: `Loading`

### `CensusManaged`
- From: `Managed`
- On: `Census`()
- Emits: `RequestRefused`
- To: `Managed`

### `CensusHeld`
- From: `Held`
- On: `Census`()
- Emits: `RequestRefused`
- To: `Held`

### `CensusRetrying`
- From: `Retrying`
- On: `Census`()
- Emits: `RequestRefused`
- To: `Retrying`

### `CensusUnloading`
- From: `Unloading`
- On: `Census`()
- Emits: `RequestRefused`
- To: `Unloading`

### `CensusGone`
- From: `Gone`
- On: `Census`()
- Emits: `RequestRefused`
- To: `Gone`

### `CensusCreating`
- From: `Creating`
- On: `Census`()
- Emits: `RequestRefused`
- To: `Creating`

### `RegisterDormant`
- From: `Dormant`
- On: `Register`(holder_ended)
- Guards:
  - `not_declared`
- Emits: `StoreRegistered`
- To: `Managed`

### `RegisterDormantLaunchOwned`
- From: `Dormant`
- On: `Register`(holder_ended)
- Guards:
  - `declared`
- Emits: `RequestRefused`
- To: `Dormant`

### `RegisterManagedHostHandle`
- From: `Managed`
- On: `Register`(holder_ended)
- Guards:
  - `a_hosts_own_mob`
- Emits: `StoreRegistered`
- To: `Managed`

### `RegisterManagedHostHandleSetRun`
- From: `Managed`
- On: `Register`(holder_ended)
- Guards:
  - `run_by_the_set`
  - `its_mob_runs`
- Emits: `RequestRefused`
- To: `Managed`

### `RegisterManagedHostHandleOverEnded`
- From: `Managed`
- On: `Register`(holder_ended)
- Guards:
  - `run_by_the_set`
  - `its_mob_has_ended`
- Emits: `StoreRegistered`
- To: `Managed`

### `RegisterManagedLaunchOwned`
- From: `Managed`
- On: `Register`(holder_ended)
- Guards:
  - `declared`
- Emits: `RequestRefused`
- To: `Managed`

### `RegisterLoading`
- From: `Loading`
- On: `Register`(holder_ended)
- Emits: `RequestRefused`
- To: `Loading`

### `RegisterHeld`
- From: `Held`
- On: `Register`(holder_ended)
- Emits: `RequestRefused`
- To: `Held`

### `RegisterQuarantined`
- From: `Quarantined`
- On: `Register`(holder_ended)
- Emits: `RequestRefused`
- To: `Quarantined`

### `RegisterRetrying`
- From: `Retrying`
- On: `Register`(holder_ended)
- Emits: `RequestRefused`
- To: `Retrying`

### `RegisterUnloading`
- From: `Unloading`
- On: `Register`(holder_ended)
- Emits: `RequestRefused`
- To: `Unloading`

### `RegisterGone`
- From: `Gone`
- On: `Register`(holder_ended)
- Emits: `RequestRefused`
- To: `Gone`

### `RegisterCreating`
- From: `Creating`
- On: `Register`(holder_ended)
- Emits: `RequestRefused`
- To: `Creating`

### `DeclareDormant`
- From: `Dormant`
- On: `Declare`(vacant)
- Guards:
  - `not_declared`
- Emits: `LaunchDeclared`
- To: `Dormant`

### `DeclareDormantDeclared`
- From: `Dormant`
- On: `Declare`(vacant)
- Guards:
  - `declared`
- Emits: `RequestRefused`
- To: `Dormant`

### `DeclareLoading`
- From: `Loading`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Loading`

### `DeclareManaged`
- From: `Managed`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Managed`

### `DeclareUnloading`
- From: `Unloading`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Unloading`

### `DeclareHeld`
- From: `Held`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Held`

### `DeclareQuarantined`
- From: `Quarantined`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Quarantined`

### `DeclareRetrying`
- From: `Retrying`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Retrying`

### `DeclareGone`
- From: `Gone`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Gone`

### `DeclareCreating`
- From: `Creating`
- On: `Declare`(vacant)
- Emits: `RequestRefused`
- To: `Creating`

### `EndLaunchWaitDormant`
- From: `Dormant`
- On: `EndLaunchWait`()
- Guards:
  - `awaits_launch`
- Emits: `LaunchWaitEnded`
- To: `Dormant`

### `EndLaunchWaitDormantNotAwaited`
- From: `Dormant`
- On: `EndLaunchWait`()
- Guards:
  - `not_awaiting_launch`
- Emits: `RequestRefused`
- To: `Dormant`

### `EndLaunchWaitLoading`
- From: `Loading`
- On: `EndLaunchWait`()
- Guards:
  - `awaits_launch`
- Emits: `LaunchWaitEnded`
- To: `Loading`

### `EndLaunchWaitLoadingNotAwaited`
- From: `Loading`
- On: `EndLaunchWait`()
- Guards:
  - `not_awaiting_launch`
- Emits: `RequestRefused`
- To: `Loading`

### `EndLaunchWaitManaged`
- From: `Managed`
- On: `EndLaunchWait`()
- Guards:
  - `awaits_launch`
- Emits: `LaunchWaitEnded`
- To: `Managed`

### `EndLaunchWaitManagedNotAwaited`
- From: `Managed`
- On: `EndLaunchWait`()
- Guards:
  - `not_awaiting_launch`
- Emits: `RequestRefused`
- To: `Managed`

### `EndLaunchWaitUnloading`
- From: `Unloading`
- On: `EndLaunchWait`()
- Emits: `RequestRefused`
- To: `Unloading`

### `EndLaunchWaitHeld`
- From: `Held`
- On: `EndLaunchWait`()
- Guards:
  - `awaits_launch`
- Emits: `LaunchWaitEnded`
- To: `Held`

### `EndLaunchWaitHeldNotAwaited`
- From: `Held`
- On: `EndLaunchWait`()
- Guards:
  - `not_awaiting_launch`
- Emits: `RequestRefused`
- To: `Held`

### `EndLaunchWaitQuarantined`
- From: `Quarantined`
- On: `EndLaunchWait`()
- Guards:
  - `awaits_launch`
- Emits: `LaunchWaitEnded`
- To: `Quarantined`

### `EndLaunchWaitQuarantinedNotAwaited`
- From: `Quarantined`
- On: `EndLaunchWait`()
- Guards:
  - `not_awaiting_launch`
- Emits: `RequestRefused`
- To: `Quarantined`

### `EndLaunchWaitRetrying`
- From: `Retrying`
- On: `EndLaunchWait`()
- Guards:
  - `awaits_launch`
- Emits: `LaunchWaitEnded`
- To: `Retrying`

### `EndLaunchWaitRetryingNotAwaited`
- From: `Retrying`
- On: `EndLaunchWait`()
- Guards:
  - `not_awaiting_launch`
- Emits: `RequestRefused`
- To: `Retrying`

### `EndLaunchWaitGone`
- From: `Gone`
- On: `EndLaunchWait`()
- Emits: `RequestRefused`
- To: `Gone`

### `EndLaunchWaitCreating`
- From: `Creating`
- On: `EndLaunchWait`()
- Guards:
  - `awaited`
- Emits: `LaunchWaitEnded`
- To: `Creating`

### `EndLaunchWaitCreatingNotAwaited`
- From: `Creating`
- On: `EndLaunchWait`()
- Guards:
  - `not_awaited`
- Emits: `RequestRefused`
- To: `Creating`

### `RequestLoadDormant`
- From: `Dormant`
- On: `RequestLoad`(trigger)
- Guards:
  - `admitted`
  - `not_a_demand`
- Emits: `LoadAdmitted`
- To: `Loading`

### `RequestLoadDormantAwaitsLaunch`
- From: `Dormant`
- On: `RequestLoad`(trigger)
- Guards:
  - `declared`
  - `lookup`
  - `awaits_launch`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestLoadDormantVacant`
- From: `Dormant`
- On: `RequestLoad`(trigger)
- Guards:
  - `declared`
  - `lookup`
  - `not_awaiting_launch`
  - `vacant`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestLoadDormantLaunchOwned`
- From: `Dormant`
- On: `RequestLoad`(trigger)
- Guards:
  - `declared`
  - `pass`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestLoadDormantDemand`
- From: `Dormant`
- On: `RequestLoad`(trigger)
- Guards:
  - `demand`
  - `serves_a_lookup`
  - `not_stopped`
- Emits: `LoadAdmitted`
- To: `Loading`

### `RequestLoadDormantStopped`
- From: `Dormant`
- On: `RequestLoad`(trigger)
- Guards:
  - `demand`
  - `serves_a_lookup`
  - `stopped`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestLoadDormantStopUnread`
- From: `Dormant`
- On: `RequestLoad`(trigger)
- Guards:
  - `demand`
  - `serves_a_lookup`
  - `unread`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestLoadLoading`
- From: `Loading`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Loading`

### `RequestLoadManaged`
- From: `Managed`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Managed`

### `RequestLoadHeld`
- From: `Held`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Held`

### `RequestLoadQuarantined`
- From: `Quarantined`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Quarantined`

### `RequestLoadRetrying`
- From: `Retrying`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Retrying`

### `RequestLoadUnloading`
- From: `Unloading`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Unloading`

### `RequestLoadGone`
- From: `Gone`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Gone`

### `RequestLoadCreating`
- From: `Creating`
- On: `RequestLoad`(trigger)
- Emits: `RequestRefused`
- To: `Creating`

### `ResolveLoadManaged`
- From: `Loading`
- On: `ResolveLoad`(outcome, class)
- Guards:
  - `managed`
- Emits: `StoreLoaded`
- To: `Managed`

### `ResolveLoadHeld`
- From: `Loading`
- On: `ResolveLoad`(outcome, class)
- Guards:
  - `held`
- Emits: `StoreLoaded`
- To: `Held`

### `ResolveLoadSettled`
- From: `Loading`
- On: `ResolveLoad`(outcome, class)
- Guards:
  - `settled`
  - `not_declared`
- Emits: `StoreLoaded`
- To: `Gone`

### `ResolveLoadSettledDeclared`
- From: `Loading`
- On: `ResolveLoad`(outcome, class)
- Guards:
  - `settled`
  - `declared`
- Emits: `StoreLoaded`
- To: `Dormant`

### `ResolveLoadFailed`
- From: `Loading`
- On: `ResolveLoad`(outcome, class)
- Guards:
  - `failed`
- Emits: `StoreQuarantined`
- To: `Quarantined`

### `ResolveLoadDormant`
- From: `Dormant`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Dormant`

### `ResolveLoadWhileManaged`
- From: `Managed`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Managed`

### `ResolveLoadWhileHeld`
- From: `Held`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Held`

### `ResolveLoadQuarantined`
- From: `Quarantined`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Quarantined`

### `ResolveLoadRetrying`
- From: `Retrying`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Retrying`

### `ResolveLoadUnloading`
- From: `Unloading`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Unloading`

### `ResolveLoadGone`
- From: `Gone`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Gone`

### `ResolveLoadCreating`
- From: `Creating`
- On: `ResolveLoad`(outcome, class)
- Emits: `RequestRefused`
- To: `Creating`

### `RequestRetryQuarantined`
- From: `Quarantined`
- On: `RequestRetry`(trigger)
- Guards:
  - `retryable`
  - `not_awaiting_launch`
- Emits: `RetryAdmitted`
- To: `Retrying`

### `RequestRetryQuarantinedNotRetryable`
- From: `Quarantined`
- On: `RequestRetry`(trigger)
- Guards:
  - `on_demand`
  - `needs_repair`
  - `not_awaiting_launch`
- Emits: `RequestRefused`
- To: `Quarantined`

### `RequestRetryQuarantinedAwaitsLaunch`
- From: `Quarantined`
- On: `RequestRetry`(trigger)
- Guards:
  - `on_demand`
  - `awaits_launch`
- Emits: `RequestRefused`
- To: `Quarantined`

### `RequestRetryRetrying`
- From: `Retrying`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Retrying`

### `RequestRetryDormant`
- From: `Dormant`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestRetryLoading`
- From: `Loading`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Loading`

### `RequestRetryManaged`
- From: `Managed`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Managed`

### `RequestRetryHeld`
- From: `Held`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Held`

### `RequestRetryUnloading`
- From: `Unloading`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Unloading`

### `RequestRetryGone`
- From: `Gone`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Gone`

### `RequestRetryCreating`
- From: `Creating`
- On: `RequestRetry`(trigger)
- Emits: `RequestRefused`
- To: `Creating`

### `ResolveRetryFailed`
- From: `Retrying`
- On: `ResolveRetry`(outcome, class)
- Guards:
  - `failed`
- Emits: `StoreStillQuarantined`
- To: `Quarantined`

### `ResolveRetryManaged`
- From: `Retrying`
- On: `ResolveRetry`(outcome, class)
- Guards:
  - `managed`
- Emits: `StoreReleased`
- To: `Managed`

### `ResolveRetryHeld`
- From: `Retrying`
- On: `ResolveRetry`(outcome, class)
- Guards:
  - `held`
- Emits: `StoreReleased`
- To: `Held`

### `ResolveRetrySettled`
- From: `Retrying`
- On: `ResolveRetry`(outcome, class)
- Guards:
  - `settled`
  - `not_declared`
- Emits: `StoreReleased`
- To: `Gone`

### `ResolveRetrySettledDeclared`
- From: `Retrying`
- On: `ResolveRetry`(outcome, class)
- Guards:
  - `settled`
  - `declared`
- Emits: `StoreReleased`
- To: `Dormant`

### `ResolveRetryDormant`
- From: `Dormant`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Dormant`

### `ResolveRetryLoading`
- From: `Loading`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Loading`

### `ResolveRetryWhileManaged`
- From: `Managed`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Managed`

### `ResolveRetryWhileHeld`
- From: `Held`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Held`

### `ResolveRetryQuarantined`
- From: `Quarantined`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Quarantined`

### `ResolveRetryUnloading`
- From: `Unloading`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Unloading`

### `ResolveRetryGone`
- From: `Gone`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Gone`

### `ResolveRetryCreating`
- From: `Creating`
- On: `ResolveRetry`(outcome, class)
- Emits: `RequestRefused`
- To: `Creating`

### `RequestUnloadManaged`
- From: `Managed`
- On: `RequestUnload`()
- Guards:
  - `reloadable`
- Emits: `UnloadAdmitted`
- To: `Unloading`

### `RequestUnloadManagedNotReloadable`
- From: `Managed`
- On: `RequestUnload`()
- Guards:
  - `not_reloadable`
- Emits: `RequestRefused`
- To: `Managed`

### `RequestUnloadUnloading`
- From: `Unloading`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Unloading`

### `RequestUnloadDormant`
- From: `Dormant`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestUnloadLoading`
- From: `Loading`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Loading`

### `RequestUnloadHeld`
- From: `Held`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Held`

### `RequestUnloadQuarantined`
- From: `Quarantined`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Quarantined`

### `RequestUnloadRetrying`
- From: `Retrying`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Retrying`

### `RequestUnloadGone`
- From: `Gone`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Gone`

### `RequestUnloadCreating`
- From: `Creating`
- On: `RequestUnload`()
- Emits: `RequestRefused`
- To: `Creating`

### `ResolveUnloadClosed`
- From: `Unloading`
- On: `ResolveUnload`(outcome, stop)
- Guards:
  - `closed`
- Emits: `StoreUnloaded`
- To: `Dormant`

### `ResolveUnloadBusy`
- From: `Unloading`
- On: `ResolveUnload`(outcome, stop)
- Guards:
  - `busy`
- Emits: `UnloadDeclined`
- To: `Managed`

### `ResolveUnloadDormant`
- From: `Dormant`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Dormant`

### `ResolveUnloadLoading`
- From: `Loading`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Loading`

### `ResolveUnloadWhileManaged`
- From: `Managed`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Managed`

### `ResolveUnloadHeld`
- From: `Held`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Held`

### `ResolveUnloadQuarantined`
- From: `Quarantined`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Quarantined`

### `ResolveUnloadRetrying`
- From: `Retrying`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Retrying`

### `ResolveUnloadGone`
- From: `Gone`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Gone`

### `ResolveUnloadCreating`
- From: `Creating`
- On: `ResolveUnload`(outcome, stop)
- Emits: `RequestRefused`
- To: `Creating`

### `ReleaseHeldManaged`
- From: `Held`
- On: `ReleaseHeld`(outcome)
- Guards:
  - `managed`
- Emits: `HoldReleased`
- To: `Managed`

### `ReleaseHeldSettled`
- From: `Held`
- On: `ReleaseHeld`(outcome)
- Guards:
  - `settled`
  - `not_declared`
- Emits: `HoldReleased`
- To: `Gone`

### `ReleaseHeldSettledDeclared`
- From: `Held`
- On: `ReleaseHeld`(outcome)
- Guards:
  - `settled`
  - `declared`
- Emits: `HoldReleased`
- To: `Dormant`

### `ReleaseHeldKept`
- From: `Held`
- On: `ReleaseHeld`(outcome)
- Guards:
  - `unresolved`
- Emits: `RequestRefused`
- To: `Held`

### `ReleaseHeldDormant`
- From: `Dormant`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Dormant`

### `ReleaseHeldLoading`
- From: `Loading`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Loading`

### `ReleaseHeldWhileManaged`
- From: `Managed`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Managed`

### `ReleaseHeldQuarantined`
- From: `Quarantined`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Quarantined`

### `ReleaseHeldRetrying`
- From: `Retrying`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Retrying`

### `ReleaseHeldUnloading`
- From: `Unloading`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Unloading`

### `ReleaseHeldGone`
- From: `Gone`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Gone`

### `ReleaseHeldCreating`
- From: `Creating`
- On: `ReleaseHeld`(outcome)
- Emits: `RequestRefused`
- To: `Creating`

### `DestroyManaged`
- From: `Managed`
- On: `Destroy`()
- Guards:
  - `not_declared`
- Emits: `StoreGone`
- To: `Gone`

### `DestroyManagedLaunchOwned`
- From: `Managed`
- On: `Destroy`()
- Guards:
  - `declared`
- Emits: `RequestRefused`
- To: `Managed`

### `DestroyDormant`
- From: `Dormant`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Dormant`

### `DestroyLoading`
- From: `Loading`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Loading`

### `DestroyHeld`
- From: `Held`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Held`

### `DestroyQuarantined`
- From: `Quarantined`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Quarantined`

### `DestroyRetrying`
- From: `Retrying`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Retrying`

### `DestroyUnloading`
- From: `Unloading`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Unloading`

### `DestroyGone`
- From: `Gone`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Gone`

### `DestroyCreating`
- From: `Creating`
- On: `Destroy`()
- Emits: `RequestRefused`
- To: `Creating`

### `RequestCreateDormant`
- From: `Dormant`
- On: `RequestCreate`(launch)
- Guards:
  - `not_declared`
  - `not_the_launch`
  - `stands_for_nothing_yet`
- Emits: `CreateAdmitted`
- To: `Creating`

### `RequestCreateDormantKnown`
- From: `Dormant`
- On: `RequestCreate`(launch)
- Guards:
  - `not_declared`
  - `not_the_launch`
  - `stands_for_a_store`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestCreateDormantNotDeclared`
- From: `Dormant`
- On: `RequestCreate`(launch)
- Guards:
  - `not_declared`
  - `the_launch`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestCreateDormantLaunch`
- From: `Dormant`
- On: `RequestCreate`(launch)
- Guards:
  - `declared`
  - `the_launch`
  - `holds_no_mob`
- Emits: `CreateAdmitted`
- To: `Creating`

### `RequestCreateDormantLaunchHoldsAMob`
- From: `Dormant`
- On: `RequestCreate`(launch)
- Guards:
  - `declared`
  - `the_launch`
  - `holds_a_mob`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestCreateDormantLaunchOwned`
- From: `Dormant`
- On: `RequestCreate`(launch)
- Guards:
  - `declared`
  - `not_the_launch`
- Emits: `RequestRefused`
- To: `Dormant`

### `RequestCreateLoading`
- From: `Loading`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Loading`

### `RequestCreateManaged`
- From: `Managed`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Managed`

### `RequestCreateUnloading`
- From: `Unloading`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Unloading`

### `RequestCreateHeld`
- From: `Held`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Held`

### `RequestCreateQuarantined`
- From: `Quarantined`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Quarantined`

### `RequestCreateRetrying`
- From: `Retrying`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Retrying`

### `RequestCreateGone`
- From: `Gone`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Gone`

### `RequestCreateCreating`
- From: `Creating`
- On: `RequestCreate`(launch)
- Emits: `RequestRefused`
- To: `Creating`

### `ResolveCreateCreated`
- From: `Creating`
- On: `ResolveCreate`(outcome, in_memory)
- Guards:
  - `created`
  - `not_declared`
  - `with_a_store_file`
- Emits: `StoreRegistered`
- To: `Managed`

### `ResolveCreateCreatedInMemory`
- From: `Creating`
- On: `ResolveCreate`(outcome, in_memory)
- Guards:
  - `created`
  - `not_declared`
  - `in_memory`
- Emits: `StoreRegistered`
- To: `Managed`

### `ResolveCreateCreatedDeclared`
- From: `Creating`
- On: `ResolveCreate`(outcome, in_memory)
- Guards:
  - `created`
  - `declared`
- Emits: `StoreRegistered`
- To: `Managed`

### `ResolveCreateFailed`
- From: `Creating`
- On: `ResolveCreate`(outcome, in_memory)
- Guards:
  - `failed`
  - `not_declared`
- Emits: `CreateReleased`
- To: `Gone`

### `ResolveCreateFailedDeclared`
- From: `Creating`
- On: `ResolveCreate`(outcome, in_memory)
- Guards:
  - `failed`
  - `declared`
- Emits: `CreateReleased`
- To: `Dormant`

### `ResolveCreateDormant`
- From: `Dormant`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Dormant`

### `ResolveCreateLoading`
- From: `Loading`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Loading`

### `ResolveCreateManaged`
- From: `Managed`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Managed`

### `ResolveCreateUnloading`
- From: `Unloading`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Unloading`

### `ResolveCreateHeld`
- From: `Held`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Held`

### `ResolveCreateQuarantined`
- From: `Quarantined`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Quarantined`

### `ResolveCreateRetrying`
- From: `Retrying`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Retrying`

### `ResolveCreateGone`
- From: `Gone`
- On: `ResolveCreate`(outcome, in_memory)
- Emits: `RequestRefused`
- To: `Gone`

### `ReadStopDormantStopped`
- From: `Dormant`
- On: `ReadStop`(stopped)
- Guards:
  - `unread`
  - `stopped`
- Emits: `StopRead`
- To: `Dormant`

### `ReadStopDormantNotStopped`
- From: `Dormant`
- On: `ReadStop`(stopped)
- Guards:
  - `unread`
  - `not_stopped`
- Emits: `StopRead`
- To: `Dormant`

### `ReadStopDormantKnown`
- From: `Dormant`
- On: `ReadStop`(stopped)
- Guards:
  - `known`
- Emits: `RequestRefused`
- To: `Dormant`

### `ReadStopLoading`
- From: `Loading`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Loading`

### `ReadStopManaged`
- From: `Managed`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Managed`

### `ReadStopUnloading`
- From: `Unloading`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Unloading`

### `ReadStopHeld`
- From: `Held`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Held`

### `ReadStopQuarantined`
- From: `Quarantined`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Quarantined`

### `ReadStopRetrying`
- From: `Retrying`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Retrying`

### `ReadStopGone`
- From: `Gone`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Gone`

### `ReadStopCreating`
- From: `Creating`
- On: `ReadStop`(stopped)
- Emits: `RequestRefused`
- To: `Creating`

## Coverage
### Code Anchors
- `mob_store_restore_lifecycle` (machine `MobStoreRestoreLifecycleMachine`): `crates/meerkat-mob/src/machines/mob_store_restore_lifecycle.rs` — MobStoreRestoreLifecycleMachine owns one mob store's stay in a host's set: whether its mob is dormant, loading, managed, unloading, held, quarantined, retrying or gone, the single load, re-attempt or unload in flight, the class a quarantine holds, whether an idle mob's store can be unloaded and loaded again, whether the mob's handle may be handed out, and for the store a host's launch declares: that it is its launch's, whether its mob still waits for the launch's first serve, and whether it holds no mob, and a create's hold of its mob's id from before anything of the mob is built until the create is resolved

### Scenarios
- `mob_store_census` — mob_store_restore_lifecycle_machine.rs::a_census_lists_a_dormant_store_and_finds_a_quarantined_one_afresh lists a dormant store, finds a quarantined one afresh with no class and no attempt, and refuses to list a store the set loads, manages, unloads, holds or re-attempts, or one that is gone
- `mob_store_registration` — mob_store_restore_lifecycle_machine.rs::a_mob_built_outside_a_load_joins_the_set_as_managed registers a host's handle as managed on a record that stands for nothing yet and on one a census listed, lets it replace the handle of a mob a host inserted, refuses it over a mob the set itself runs while that mob runs and admits it once that mob has ended, registers nothing while an unload is admitted, and refuses every other phase
- `mob_store_single_flight_load` — mob_store_restore_lifecycle_machine.rs::one_load_is_in_flight_at_a_time admits one load of a dormant store, refuses a second request while it runs, answers a request that meets an admitted unload with the refusal its caller waits on, and refuses a request in every phase that has nothing to load
- `mob_store_load_outcomes` — mob_store_restore_lifecycle_machine.rs::a_load_resolves_into_the_phase_its_outcome_names resolves a load into managed, held or gone, quarantines a failed one under the class it met without counting an attempt, and refuses an outcome with no load in flight
- `mob_store_single_flight_retry` — mob_store_restore_lifecycle_machine.rs::one_re_attempt_is_in_flight_at_a_time admits one re-attempt of a quarantined store, refuses a second request while it runs without waiting, and refuses a request for a store that is not quarantined
- `mob_store_retry_eligibility` — mob_store_restore_lifecycle_machine.rs::an_on_demand_re_attempt_is_admitted_only_for_a_store_that_could_not_serve admits an on-demand re-attempt only for a store quarantined as store_unavailable, refuses it unchanged for store_unreadable and resume_refused, and admits an explicit re-attempt for every class
- `mob_store_retry_outcomes` — mob_store_restore_lifecycle_machine.rs::a_re_attempt_releases_the_store_or_keeps_it_quarantined releases the store into managed, held or gone with no class left, keeps it quarantined under the class a failed attempt met, counts every ended attempt, and refuses an outcome with no attempt in flight
- `mob_store_unload_admission` — mob_store_restore_lifecycle_machine.rs::an_unload_is_admitted_only_for_a_store_the_set_can_load_again admits the idle question for a managed persisted store before the actor is asked and stops handing the mob out, never admits it for a volatile store or a host's own handle, admits one unload at a time, and refuses every phase that is not managed
- `mob_store_unload_outcomes` — mob_store_restore_lifecycle_machine.rs::an_admitted_unload_resolves_as_the_actor_answered returns the store to dormant when the actor closed idle, so it loads again, returns the mob to managed and hands it out again when the actor was busy, and refuses an answer with no unload admitted
- `mob_store_handed_out` — mob_store_restore_lifecycle_machine.rs::a_mob_is_handed_out_only_while_its_record_is_managed hands a mob out in the managed phase alone, whichever way the record got there, and stops at an admitted unload and at a destroy
- `mob_store_held_release` — mob_store_restore_lifecycle_machine.rs::a_held_mob_leaves_its_hold_in_one_step releases a held mob into managed or gone in one step, keeps it held when the release did not resolve the hold, and refuses every phase that is not held
- `mob_store_destroy` — mob_store_restore_lifecycle_machine.rs::a_destroyed_mob_is_gone_for_good records a destroyed managed mob as gone, answers a destroy that meets an admitted unload with the refusal its caller waits on, and refuses a destroy in every other phase
- `mob_store_declared_launch` — mob_store_restore_lifecycle_machine.rs::a_declared_store_waits_for_its_launch_and_stays_its_launchs declares a store once, refuses a lookup's load and an on-demand re-attempt until the wait for the launch ends, never admits a pass's load, ends the wait once in whatever phase the serve left the record, refuses every create, handle and destroy but the launch's own, and keeps the record of a declared store that holds no mob as dormant and vacant where a store under the root is gone
- `mob_store_create_takes_the_id` — mob_store_restore_lifecycle_machine.rs::a_create_takes_its_mobs_id_before_the_mob_is_built admits a create on the record the set files for it and on the declared record that holds no mob for the launch's create alone, refuses a create under a record that stands for a store or a mob the set knows and under one whose create is in flight, resolves a created mob into managed under the origin its create gives it, and frees the id of a create that ended without a mob
- `mob_store_create_only_on_a_record_that_stands_for_nothing` — mob_store_restore_lifecycle_machine.rs::a_record_that_stands_for_a_store_refuses_a_create admits a create on the record filed for it, which has admitted nothing before, and refuses it, changing nothing, on a record a census listed, on one whose mob was loaded and unloaded again, and on one whose mob the set created and unloaded
- `mob_store_create_in_flight` — mob_store_restore_lifecycle_machine.rs::nothing_else_moves_a_record_whose_create_is_in_flight refuses a census, a registration (a host's handle included), a declaration, a load, a re-attempt, an unload, a release and a destroy of a record whose create is in flight, each unchanged, and ends the wait for the launch once while the launch's create runs
- `mob_store_stop_of_a_mob_that_is_not_loaded` — mob_store_restore_lifecycle_machine.rs::a_mob_that_is_not_loaded_keeps_its_stop_and_no_demand_loads_a_stopped_one keeps what the mob answered about its stop when its actor closed at an unload, takes a read of the stored stop only while dormant and unread, refuses a demand's load of a stopped mob and of one whose stop is unread while a lookup, a pass and the launch load it, and clears the stop in every step that leaves dormant and loading
