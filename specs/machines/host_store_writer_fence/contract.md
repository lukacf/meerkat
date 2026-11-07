# HostStoreWriterFenceMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::host_store_writer_fence`

## State
- Phase enum: `Unopened | Live | Stopped | Acquiring | Proving | Resuming | WaitingOut | Lost`
- `token`: `u64`
- `granted`: `Option<u64>`
- `own_grant`: `Option<u64>`
- `step`: `WriterFenceAcquireStep`
- `timed_out`: `u64`
- `waited_holder`: `Option<u64>`
- `stop_cause`: `WriterFenceStopCause`
- `lost_cause`: `WriterFenceLostCause`
- `writer_stopped`: `Bool`
- `open_listing_failed`: `Bool`
- `quarantined_keys`: `Set<String>`
- `quarantine_versions`: `Map<String, u64>`
- `quarantine_commit_ids`: `Map<String, Set<String>>`
- `poisoned_scopes`: `Map<String, ScopePoisonCause>`

## Inputs
- `Open`(token: u64)
- `WindowClosed`
- `RenewAnswered`(outcome: WriterFenceRenewOutcome, renewed: u64, current: u64, in_window: Bool)
- `BeginReacquire`
- `AcquireAnswered`(outcome: WriterFenceAcquireOutcome, token: u64, lease_moved: Bool)
- `LeaseEnded`
- `ListingChecked`(outcome: WriterFenceListingOutcome, token: u64)
- `Poison`(cause: WriterFenceLostCause)
- `OpenListingFailed`
- `ScopeQuarantined`(key: String, name: String, version: u64, commit_ids: Set<String>)
- `ScopePoisoned`(scope: String, cause: ScopePoisonCause)

## Signals

## Effects
- `WriterOpened`(token: u64)
- `WindowAnchored`(token: u64)
- `WriterStopped`(cause: WriterFenceStopCause)
- `AcquireRequested`(step: WriterFenceAcquireStep)
- `GrantReceived`(token: u64)
- `TokenInstalled`(token: u64)
- `WriterResumed`(token: u64)
- `HolderWaitedOut`(holder: u64)
- `RealmLost`(cause: WriterFenceLostCause)
- `RenewIgnored`
- `RequestRefused`(reason: WriterFenceRefusal)
- `ScopeQuarantineRecorded`(key: String, name: String)
- `ScopePoisonRecorded`(scope: String, cause: ScopePoisonCause)

## Invariants
- `unopened_holds_nothing`
- `no_scope_is_quarantined_and_poisoned`
- `quarantine_records_are_whole`
- `live_writer_is_not_stopped`
- `stopped_and_attempting_writer_is_stopped`
- `proving_and_resuming_carry_their_grant`
- `grant_carried_only_while_proving_or_resuming`
- `own_grant_is_above_the_token`
- `waiting_out_follows_a_timed_out_acquire`
- `waited_holder_needs_a_timed_out_acquire`

## Transitions
### `Open`
- From: `Unopened`
- On: `Open`(token)
- Emits: `WriterOpened`
- To: `Live`

### `OpenAgainLive`
- From: `Live`
- On: `Open`(token)
- Emits: `RequestRefused`
- To: `Live`

### `OpenAgainStopped`
- From: `Stopped`
- On: `Open`(token)
- Emits: `RequestRefused`
- To: `Stopped`

### `OpenAgainAcquiring`
- From: `Acquiring`
- On: `Open`(token)
- Emits: `RequestRefused`
- To: `Acquiring`

### `OpenAgainProving`
- From: `Proving`
- On: `Open`(token)
- Emits: `RequestRefused`
- To: `Proving`

### `OpenAgainResuming`
- From: `Resuming`
- On: `Open`(token)
- Emits: `RequestRefused`
- To: `Resuming`

### `OpenAgainWaitingOut`
- From: `WaitingOut`
- On: `Open`(token)
- Emits: `RequestRefused`
- To: `WaitingOut`

### `OpenLost`
- From: `Lost`
- On: `Open`(token)
- Emits: `RequestRefused`
- To: `Lost`

### `WindowClosedLive`
- From: `Live`
- On: `WindowClosed`()
- Emits: `WriterStopped`
- To: `Stopped`

### `WindowClosedNotLiveStopped`
- From: `Stopped`
- On: `WindowClosed`()
- To: `Stopped`

### `WindowClosedNotLiveAcquiring`
- From: `Acquiring`
- On: `WindowClosed`()
- To: `Acquiring`

### `WindowClosedNotLiveProving`
- From: `Proving`
- On: `WindowClosed`()
- To: `Proving`

### `WindowClosedNotLiveResuming`
- From: `Resuming`
- On: `WindowClosed`()
- To: `Resuming`

### `WindowClosedNotLiveWaitingOut`
- From: `WaitingOut`
- On: `WindowClosed`()
- To: `WaitingOut`

### `WindowClosedUnopened`
- From: `Unopened`
- On: `WindowClosed`()
- Emits: `RequestRefused`
- To: `Unopened`

### `WindowClosedLost`
- From: `Lost`
- On: `WindowClosed`()
- Emits: `RequestRefused`
- To: `Lost`

### `RenewedInWindow`
- From: `Live`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `renewed`
  - `started_inside_the_window`
- Emits: `WindowAnchored`
- To: `Live`

### `RenewedAfterTheWindow`
- From: `Live`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `renewed`
  - `started_after_the_window_closed`
- Emits: `WriterStopped`
- To: `Stopped`

### `RenewStaleLive`
- From: `Live`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `fenced_out`
  - `superseded_by_this_writer`
- Emits: `RenewIgnored`
- To: `Live`

### `RenewFencedOutLive`
- From: `Live`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `fenced_out`
  - `superseded_by_another`
- Emits: `RealmLost`
- To: `Lost`

### `RenewFailedLive`
- From: `Live`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `failed`
- Emits: `WriterStopped`
- To: `Stopped`

### `RenewTimedOutLive`
- From: `Live`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `timed_out`
- Emits: `WriterStopped`
- To: `Stopped`

### `RenewAnsweredStopped`
- From: `Stopped`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `not_fenced_out`
- Emits: `RenewIgnored`
- To: `Stopped`

### `RenewStaleStopped`
- From: `Stopped`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `fenced_out`
  - `superseded_by_this_writer`
- Emits: `RenewIgnored`
- To: `Stopped`

### `RenewFencedOutStopped`
- From: `Stopped`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `fenced_out`
  - `superseded_by_another`
- Emits: `RealmLost`
- To: `Lost`

### `ResumeRenewed`
- From: `Resuming`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `renewed`
- Emits: `WriterResumed`
- To: `Live`

### `ResumeFencedOut`
- From: `Resuming`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `fenced_out`
- Emits: `RealmLost`
- To: `Lost`

### `ResumeRenewFailed`
- From: `Resuming`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `failed`
- Emits: `WriterStopped`
- To: `Stopped`

### `ResumeRenewTimedOut`
- From: `Resuming`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Guards:
  - `timed_out`
- Emits: `WriterStopped`
- To: `Stopped`

### `RenewAnsweredNoRenewUnopened`
- From: `Unopened`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Emits: `RequestRefused`
- To: `Unopened`

### `RenewAnsweredNoRenewAcquiring`
- From: `Acquiring`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Emits: `RequestRefused`
- To: `Acquiring`

### `RenewAnsweredNoRenewProving`
- From: `Proving`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Emits: `RequestRefused`
- To: `Proving`

### `RenewAnsweredNoRenewWaitingOut`
- From: `WaitingOut`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Emits: `RequestRefused`
- To: `WaitingOut`

### `RenewAnsweredLost`
- From: `Lost`
- On: `RenewAnswered`(outcome, renewed, current, in_window)
- Emits: `RequestRefused`
- To: `Lost`

### `BeginReacquire`
- From: `Stopped`
- On: `BeginReacquire`()
- Guards:
  - `the_open_listed`
- Emits: `AcquireRequested`
- To: `Acquiring`

### `BeginReacquireAfterFailedOpen`
- From: `Stopped`
- On: `BeginReacquire`()
- Guards:
  - `the_open_listing_failed`
- Emits: `RequestRefused`
- To: `Stopped`

### `BeginReacquireLive`
- From: `Live`
- On: `BeginReacquire`()
- Emits: `RequestRefused`
- To: `Live`

### `BeginReacquireInFlightAcquiring`
- From: `Acquiring`
- On: `BeginReacquire`()
- Emits: `RequestRefused`
- To: `Acquiring`

### `BeginReacquireInFlightProving`
- From: `Proving`
- On: `BeginReacquire`()
- Emits: `RequestRefused`
- To: `Proving`

### `BeginReacquireInFlightResuming`
- From: `Resuming`
- On: `BeginReacquire`()
- Emits: `RequestRefused`
- To: `Resuming`

### `BeginReacquireWaitingOut`
- From: `WaitingOut`
- On: `BeginReacquire`()
- Emits: `RequestRefused`
- To: `WaitingOut`

### `BeginReacquireUnopened`
- From: `Unopened`
- On: `BeginReacquire`()
- Emits: `RequestRefused`
- To: `Unopened`

### `BeginReacquireLost`
- From: `Lost`
- On: `BeginReacquire`()
- Emits: `RequestRefused`
- To: `Lost`

### `AcquireGranted`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `granted`
  - `above_the_installed_token`
- Emits: `GrantReceived`
- To: `Proving`

### `AcquireGrantedOutOfOrder`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `granted`
  - `at_or_below_the_installed_token`
- Emits: `RealmLost`
- To: `Lost`

### `AcquireHeldByOwnLease`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `held`
  - `exclusive`
  - `own_lease`
- Emits: `AcquireRequested`
- To: `Acquiring`

### `AcquireHeldByAnother`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `held`
  - `exclusive`
  - `unknown_holder`
  - `no_acquire_timed_out`
- Emits: `RealmLost`
- To: `Lost`

### `AcquireHeldByRenewedHolder`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `held`
  - `exclusive`
  - `unknown_holder`
  - `an_acquire_timed_out`
  - `the_waited_holder`
  - `its_lease_moved`
- Emits: `RealmLost`
- To: `Lost`

### `AcquireHeldByUnrenewedHolder`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `held`
  - `exclusive`
  - `unknown_holder`
  - `an_acquire_timed_out`
  - `the_waited_holder`
  - `its_lease_did_not_move`
- Emits: `HolderWaitedOut`
- To: `WaitingOut`

### `AcquireHeldByPossibleLateGrant`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `held`
  - `exclusive`
  - `unknown_holder`
  - `an_acquire_timed_out`
  - `not_the_waited_holder`
- Emits: `HolderWaitedOut`
- To: `WaitingOut`

### `AcquireHeldAtTakeover`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `held`
  - `takeover`
- Emits: `WriterStopped`
- To: `Stopped`

### `AcquireFailed`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `failed`
- Emits: `WriterStopped`
- To: `Stopped`

### `AcquireTimedOut`
- From: `Acquiring`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Guards:
  - `timed_out`
- Emits: `WriterStopped`
- To: `Stopped`

### `AcquireAnsweredNotAcquiringUnopened`
- From: `Unopened`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Emits: `RequestRefused`
- To: `Unopened`

### `AcquireAnsweredNotAcquiringLive`
- From: `Live`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Emits: `RequestRefused`
- To: `Live`

### `AcquireAnsweredNotAcquiringStopped`
- From: `Stopped`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Emits: `RequestRefused`
- To: `Stopped`

### `AcquireAnsweredNotAcquiringProving`
- From: `Proving`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Emits: `RequestRefused`
- To: `Proving`

### `AcquireAnsweredNotAcquiringResuming`
- From: `Resuming`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Emits: `RequestRefused`
- To: `Resuming`

### `AcquireAnsweredNotAcquiringWaitingOut`
- From: `WaitingOut`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Emits: `RequestRefused`
- To: `WaitingOut`

### `AcquireAnsweredLost`
- From: `Lost`
- On: `AcquireAnswered`(outcome, token, lease_moved)
- Emits: `RequestRefused`
- To: `Lost`

### `LeaseEnded`
- From: `WaitingOut`
- On: `LeaseEnded`()
- To: `Stopped`

### `LeaseEndedNotWaitingOutUnopened`
- From: `Unopened`
- On: `LeaseEnded`()
- Emits: `RequestRefused`
- To: `Unopened`

### `LeaseEndedNotWaitingOutLive`
- From: `Live`
- On: `LeaseEnded`()
- Emits: `RequestRefused`
- To: `Live`

### `LeaseEndedNotWaitingOutStopped`
- From: `Stopped`
- On: `LeaseEnded`()
- Emits: `RequestRefused`
- To: `Stopped`

### `LeaseEndedNotWaitingOutAcquiring`
- From: `Acquiring`
- On: `LeaseEnded`()
- Emits: `RequestRefused`
- To: `Acquiring`

### `LeaseEndedNotWaitingOutProving`
- From: `Proving`
- On: `LeaseEnded`()
- Emits: `RequestRefused`
- To: `Proving`

### `LeaseEndedNotWaitingOutResuming`
- From: `Resuming`
- On: `LeaseEnded`()
- Emits: `RequestRefused`
- To: `Resuming`

### `LeaseEndedLost`
- From: `Lost`
- On: `LeaseEnded`()
- Emits: `RequestRefused`
- To: `Lost`

### `ListingClean`
- From: `Proving`
- On: `ListingChecked`(outcome, token)
- Guards:
  - `the_attempts_grant`
  - `clean`
- Emits: `TokenInstalled`
- To: `Resuming`

### `ListingForeign`
- From: `Proving`
- On: `ListingChecked`(outcome, token)
- Guards:
  - `the_attempts_grant`
  - `foreign_batch`
- Emits: `RealmLost`
- To: `Lost`

### `ListingFailed`
- From: `Proving`
- On: `ListingChecked`(outcome, token)
- Guards:
  - `the_attempts_grant`
  - `failed`
- Emits: `WriterStopped`
- To: `Stopped`

### `ListingOtherGrant`
- From: `Proving`
- On: `ListingChecked`(outcome, token)
- Guards:
  - `another_grant`
- Emits: `RequestRefused`
- To: `Proving`

### `ListingCheckedNotProvingUnopened`
- From: `Unopened`
- On: `ListingChecked`(outcome, token)
- Emits: `RequestRefused`
- To: `Unopened`

### `ListingCheckedNotProvingLive`
- From: `Live`
- On: `ListingChecked`(outcome, token)
- Emits: `RequestRefused`
- To: `Live`

### `ListingCheckedNotProvingStopped`
- From: `Stopped`
- On: `ListingChecked`(outcome, token)
- Emits: `RequestRefused`
- To: `Stopped`

### `ListingCheckedNotProvingAcquiring`
- From: `Acquiring`
- On: `ListingChecked`(outcome, token)
- Emits: `RequestRefused`
- To: `Acquiring`

### `ListingCheckedNotProvingResuming`
- From: `Resuming`
- On: `ListingChecked`(outcome, token)
- Emits: `RequestRefused`
- To: `Resuming`

### `ListingCheckedNotProvingWaitingOut`
- From: `WaitingOut`
- On: `ListingChecked`(outcome, token)
- Emits: `RequestRefused`
- To: `WaitingOut`

### `ListingCheckedLost`
- From: `Lost`
- On: `ListingChecked`(outcome, token)
- Emits: `RequestRefused`
- To: `Lost`

### `Poison`
- From: `Live`, `Stopped`, `Acquiring`, `Proving`, `Resuming`, `WaitingOut`
- On: `Poison`(cause)
- Emits: `RealmLost`
- To: `Lost`

### `PoisonUnopened`
- From: `Unopened`
- On: `Poison`(cause)
- Emits: `RequestRefused`
- To: `Unopened`

### `PoisonLost`
- From: `Lost`
- On: `Poison`(cause)
- Emits: `RequestRefused`
- To: `Lost`

### `OpenListingFailedLive`
- From: `Live`
- On: `OpenListingFailed`()
- To: `Live`

### `OpenListingFailedStopped`
- From: `Stopped`
- On: `OpenListingFailed`()
- To: `Stopped`

### `OpenListingFailedAcquiring`
- From: `Acquiring`
- On: `OpenListingFailed`()
- To: `Acquiring`

### `OpenListingFailedProving`
- From: `Proving`
- On: `OpenListingFailed`()
- To: `Proving`

### `OpenListingFailedResuming`
- From: `Resuming`
- On: `OpenListingFailed`()
- To: `Resuming`

### `OpenListingFailedWaitingOut`
- From: `WaitingOut`
- On: `OpenListingFailed`()
- To: `WaitingOut`

### `OpenListingFailedUnopened`
- From: `Unopened`
- On: `OpenListingFailed`()
- Emits: `RequestRefused`
- To: `Unopened`

### `OpenListingFailedLost`
- From: `Lost`
- On: `OpenListingFailed`()
- Emits: `RequestRefused`
- To: `Lost`

### `ScopeQuarantinedLive`
- From: `Live`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `not_poisoned`
- Emits: `ScopeQuarantineRecorded`
- To: `Live`

### `ScopeQuarantinedStopped`
- From: `Stopped`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `not_poisoned`
- Emits: `ScopeQuarantineRecorded`
- To: `Stopped`

### `ScopeQuarantinedAcquiring`
- From: `Acquiring`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `not_poisoned`
- Emits: `ScopeQuarantineRecorded`
- To: `Acquiring`

### `ScopeQuarantinedProving`
- From: `Proving`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `not_poisoned`
- Emits: `ScopeQuarantineRecorded`
- To: `Proving`

### `ScopeQuarantinedResuming`
- From: `Resuming`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `not_poisoned`
- Emits: `ScopeQuarantineRecorded`
- To: `Resuming`

### `ScopeQuarantinedWaitingOut`
- From: `WaitingOut`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `not_poisoned`
- Emits: `ScopeQuarantineRecorded`
- To: `WaitingOut`

### `ScopeQuarantinedAgainLive`
- From: `Live`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_recorded_incarnation`
- To: `Live`

### `ScopeQuarantinedAgainStopped`
- From: `Stopped`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_recorded_incarnation`
- To: `Stopped`

### `ScopeQuarantinedAgainAcquiring`
- From: `Acquiring`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_recorded_incarnation`
- To: `Acquiring`

### `ScopeQuarantinedAgainProving`
- From: `Proving`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_recorded_incarnation`
- To: `Proving`

### `ScopeQuarantinedAgainResuming`
- From: `Resuming`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_recorded_incarnation`
- To: `Resuming`

### `ScopeQuarantinedAgainWaitingOut`
- From: `WaitingOut`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_recorded_incarnation`
- To: `WaitingOut`

### `ScopeQuarantinedPoisonedLive`
- From: `Live`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `poisoned`
- Emits: `RequestRefused`
- To: `Live`

### `ScopeQuarantinedPoisonedStopped`
- From: `Stopped`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `poisoned`
- Emits: `RequestRefused`
- To: `Stopped`

### `ScopeQuarantinedPoisonedAcquiring`
- From: `Acquiring`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `poisoned`
- Emits: `RequestRefused`
- To: `Acquiring`

### `ScopeQuarantinedPoisonedProving`
- From: `Proving`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `poisoned`
- Emits: `RequestRefused`
- To: `Proving`

### `ScopeQuarantinedPoisonedResuming`
- From: `Resuming`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `poisoned`
- Emits: `RequestRefused`
- To: `Resuming`

### `ScopeQuarantinedPoisonedWaitingOut`
- From: `WaitingOut`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Guards:
  - `a_new_incarnation`
  - `poisoned`
- Emits: `RequestRefused`
- To: `WaitingOut`

### `ScopeQuarantinedUnopened`
- From: `Unopened`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Emits: `RequestRefused`
- To: `Unopened`

### `ScopeQuarantinedLost`
- From: `Lost`
- On: `ScopeQuarantined`(key, name, version, commit_ids)
- Emits: `RequestRefused`
- To: `Lost`

### `ScopePoisonedLive`
- From: `Live`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `not_quarantined`
- Emits: `ScopePoisonRecorded`
- To: `Live`

### `ScopePoisonedStopped`
- From: `Stopped`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `not_quarantined`
- Emits: `ScopePoisonRecorded`
- To: `Stopped`

### `ScopePoisonedAcquiring`
- From: `Acquiring`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `not_quarantined`
- Emits: `ScopePoisonRecorded`
- To: `Acquiring`

### `ScopePoisonedProving`
- From: `Proving`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `not_quarantined`
- Emits: `ScopePoisonRecorded`
- To: `Proving`

### `ScopePoisonedResuming`
- From: `Resuming`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `not_quarantined`
- Emits: `ScopePoisonRecorded`
- To: `Resuming`

### `ScopePoisonedWaitingOut`
- From: `WaitingOut`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `not_quarantined`
- Emits: `ScopePoisonRecorded`
- To: `WaitingOut`

### `ScopePoisonedAgainLive`
- From: `Live`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `poisoned`
- To: `Live`

### `ScopePoisonedAgainStopped`
- From: `Stopped`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `poisoned`
- To: `Stopped`

### `ScopePoisonedAgainAcquiring`
- From: `Acquiring`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `poisoned`
- To: `Acquiring`

### `ScopePoisonedAgainProving`
- From: `Proving`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `poisoned`
- To: `Proving`

### `ScopePoisonedAgainResuming`
- From: `Resuming`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `poisoned`
- To: `Resuming`

### `ScopePoisonedAgainWaitingOut`
- From: `WaitingOut`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `poisoned`
- To: `WaitingOut`

### `ScopePoisonedQuarantinedLive`
- From: `Live`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `quarantined`
- Emits: `RequestRefused`
- To: `Live`

### `ScopePoisonedQuarantinedStopped`
- From: `Stopped`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `quarantined`
- Emits: `RequestRefused`
- To: `Stopped`

### `ScopePoisonedQuarantinedAcquiring`
- From: `Acquiring`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `quarantined`
- Emits: `RequestRefused`
- To: `Acquiring`

### `ScopePoisonedQuarantinedProving`
- From: `Proving`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `quarantined`
- Emits: `RequestRefused`
- To: `Proving`

### `ScopePoisonedQuarantinedResuming`
- From: `Resuming`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `quarantined`
- Emits: `RequestRefused`
- To: `Resuming`

### `ScopePoisonedQuarantinedWaitingOut`
- From: `WaitingOut`
- On: `ScopePoisoned`(scope, cause)
- Guards:
  - `not_poisoned`
  - `quarantined`
- Emits: `RequestRefused`
- To: `WaitingOut`

### `ScopePoisonedUnopened`
- From: `Unopened`
- On: `ScopePoisoned`(scope, cause)
- Emits: `RequestRefused`
- To: `Unopened`

### `ScopePoisonedLost`
- From: `Lost`
- On: `ScopePoisoned`(scope, cause)
- Emits: `RequestRefused`
- To: `Lost`

## Coverage
### Code Anchors
- `host_store_writer_fence` (machine `HostStoreWriterFenceMachine`): `crates/meerkat-runtime/src/store/spi/writer_fence.rs` — HostStoreWriterFenceMachine owns the writer fence of one host store realm: the writer's phase, the token it commits with, the grant it has not installed, the acquires it gave up on, the holder it waits out, and why it stopped or lost the realm

### Scenarios
- `writer_fence_open` — host_store_writer_fence_machine.rs::an_open_installs_its_grant_once makes the writer live under the open's token, whatever token the store answered, and refuses a second open in every later phase
- `writer_fence_stop` — host_store_writer_fence_machine.rs::a_live_writer_stops_on_a_failed_renew_or_a_closed_window anchors the window on a renew that started inside it, and stops the writer on a failed or timed-out renew, a renew that started after the window closed, or a window seen closed
- `writer_fence_renew_answers` — host_store_writer_fence_machine.rs::renew_answers_never_resume_a_stopped_writer ignores a renew that answers once the writer stopped and a fenced-out answer naming the writer's own newer token, loses the realm to any other fenced-out answer, and refuses a renew answer where no renew can be in flight
- `writer_fence_resume` — host_store_writer_fence_machine.rs::a_stopped_writer_resumes_only_through_a_proved_grant re-acquires from Stopped alone, takes its own recorded lease over, proves the grant with a clean listing, installs its token and resumes on its first renew, and refuses a re-acquire while live, in flight, unopened or lost
- `writer_fence_failed_attempts` — host_store_writer_fence_machine.rs::a_failed_attempt_leaves_the_writer_stopped returns a failed, timed-out or held takeover acquire, a failed listing and a failed resume renew to Stopped, counts a timed-out acquire, and keeps a grant whose listing failed as the writer's own lease, which the next attempt takes over
- `writer_fence_open_listing_failed` — host_store_writer_fence_machine.rs::a_realm_whose_open_listing_failed_never_reacquires records that the open's listing failed in every opened phase, refuses it unopened or lost, and refuses every later re-acquire of the stopped writer, changing nothing
- `writer_fence_scope_classification` — host_store_writer_fence_machine.rs::a_quarantine_records_each_conflicted_incarnation_under_its_key, a_scope_poison_keeps_its_first_cause, a_scope_is_quarantined_or_poisoned_never_both, scope_classifications_survive_a_stop_a_reacquire_and_a_resume and scope_classifications_are_refused_unopened_or_lost record a key and each conflicted incarnation once, keep the first poison cause, refuse a second classification, keep both through a stop, a re-acquire and a resume, and refuse both unopened or lost, in every opened phase
- `writer_fence_unknown_holders` — host_store_writer_fence_machine.rs::an_unknown_holder_is_waited_out_only_after_a_timed_out_acquire loses the realm to an unknown holder with no acquire timed out, waits out one while an acquire had timed out, refuses a re-acquire until its lease ended, loses the realm when the same holder still holds the fence with its lease moved later, and waits out again one whose lease did not move
- `writer_fence_loss` — host_store_writer_fence_machine.rs::the_realm_is_lost_once_and_for_good loses the realm to a foreign batch, a fenced-out resume renew, a grant at or below the installed token and a poison, keeps the first cause, and refuses every later input and every answer the writer did not ask for
