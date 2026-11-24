# MobHostedSetLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::mob_hosted_set_lifecycle`

## State
- Phase enum: `Unrestored | Attempted | Restored`
- `census_owed`: `Bool`
- `census_complete`: `Bool`
- `host_inserted`: `Bool`
- `all_mobs_known`: `Bool`

## Inputs
- `ExpectCensus`(owed: Bool)
- `BeginPass`
- `FinishPass`
- `DeclareRestored`
- `RegisterHostHandle`

## Signals

## Effects
- `CensusExpected`(owed: Bool)
- `PassBegun`(first: Bool)
- `PassFinished`
- `CensusCompleted`
- `HostHandleNoted`
- `RequestRefused`(reason: MobHostedSetRefusal)

## Invariants
- `a_restored_set_has_its_census`
- `a_pass_is_begun_only_where_a_census_is_owed`
- `all_mobs_are_known_exactly_when_none_can_still_turn_up`

## Transitions
### `ExpectCensusOwed`
- From: `Unrestored`
- On: `ExpectCensus`(owed)
- Guards:
  - `owed`
  - `census_not_complete`
- Emits: `CensusExpected`
- To: `Unrestored`

### `ExpectCensusOwedComplete`
- From: `Unrestored`
- On: `ExpectCensus`(owed)
- Guards:
  - `owed`
  - `census_complete`
- Emits: `CensusExpected`
- To: `Unrestored`

### `ExpectCensusNotOwed`
- From: `Unrestored`
- On: `ExpectCensus`(owed)
- Guards:
  - `not_owed`
  - `nothing_can_turn_up`
- Emits: `CensusExpected`
- To: `Unrestored`

### `ExpectCensusNotOwedHostInserts`
- From: `Unrestored`
- On: `ExpectCensus`(owed)
- Guards:
  - `not_owed`
  - `a_host_inserts`
- Emits: `CensusExpected`
- To: `Unrestored`

### `ExpectCensusAttempted`
- From: `Attempted`
- On: `ExpectCensus`(owed)
- Emits: `RequestRefused`
- To: `Attempted`

### `ExpectCensusRestored`
- From: `Restored`
- On: `ExpectCensus`(owed)
- Emits: `RequestRefused`
- To: `Restored`

### `BeginPassFirst`
- From: `Unrestored`
- On: `BeginPass`()
- Guards:
  - `census_owed`
- Emits: `PassBegun`
- To: `Attempted`

### `BeginPassNoCensusOwed`
- From: `Unrestored`
- On: `BeginPass`()
- Guards:
  - `no_census_owed`
- Emits: `RequestRefused`
- To: `Unrestored`

### `BeginPassAgain`
- From: `Attempted`
- On: `BeginPass`()
- Emits: `PassBegun`
- To: `Attempted`

### `BeginPassRestored`
- From: `Restored`
- On: `BeginPass`()
- Emits: `RequestRefused`
- To: `Restored`

### `FinishPassCompletesCensus`
- From: `Attempted`
- On: `FinishPass`()
- Guards:
  - `census_not_complete`
- Emits: `PassFinished`, `CensusCompleted`
- To: `Restored`

### `FinishPassCensusComplete`
- From: `Attempted`
- On: `FinishPass`()
- Guards:
  - `census_complete`
- Emits: `PassFinished`
- To: `Restored`

### `FinishPassUnrestored`
- From: `Unrestored`
- On: `FinishPass`()
- Emits: `RequestRefused`
- To: `Unrestored`

### `FinishPassRestored`
- From: `Restored`
- On: `FinishPass`()
- Emits: `RequestRefused`
- To: `Restored`

### `DeclareRestoredUnrestored`
- From: `Unrestored`
- On: `DeclareRestored`()
- Guards:
  - `census_not_complete`
- Emits: `CensusCompleted`
- To: `Unrestored`

### `DeclareRestoredUnrestoredComplete`
- From: `Unrestored`
- On: `DeclareRestored`()
- Guards:
  - `census_complete`
- Emits: `RequestRefused`
- To: `Unrestored`

### `DeclareRestoredAttempted`
- From: `Attempted`
- On: `DeclareRestored`()
- Guards:
  - `census_not_complete`
- Emits: `CensusCompleted`
- To: `Attempted`

### `DeclareRestoredAttemptedComplete`
- From: `Attempted`
- On: `DeclareRestored`()
- Guards:
  - `census_complete`
- Emits: `RequestRefused`
- To: `Attempted`

### `DeclareRestoredRestored`
- From: `Restored`
- On: `DeclareRestored`()
- Emits: `RequestRefused`
- To: `Restored`

### `RegisterHostHandleUnrestored`
- From: `Unrestored`
- On: `RegisterHostHandle`()
- Guards:
  - `census_not_complete`
- Emits: `HostHandleNoted`
- To: `Unrestored`

### `RegisterHostHandleUnrestoredComplete`
- From: `Unrestored`
- On: `RegisterHostHandle`()
- Guards:
  - `census_complete`
- Emits: `HostHandleNoted`
- To: `Unrestored`

### `RegisterHostHandleAttempted`
- From: `Attempted`
- On: `RegisterHostHandle`()
- Guards:
  - `census_not_complete`
- Emits: `HostHandleNoted`
- To: `Attempted`

### `RegisterHostHandleAttemptedComplete`
- From: `Attempted`
- On: `RegisterHostHandle`()
- Guards:
  - `census_complete`
- Emits: `HostHandleNoted`
- To: `Attempted`

### `RegisterHostHandleRestored`
- From: `Restored`
- On: `RegisterHostHandle`()
- Emits: `HostHandleNoted`
- To: `Restored`

## Coverage
### Code Anchors
- `mob_hosted_set_lifecycle` (machine `MobHostedSetLifecycleMachine`): `crates/meerkat-mob/src/machines/mob_hosted_set_lifecycle.rs` — MobHostedSetLifecycleMachine owns the facts about a host's mob set as a whole, which no store's record can hold: how far the pass over the stores the host has from before came (not begun, begun and not finished, finished), whether a census of such stores is owed, whether the census is complete (a pass finished, or the host declared its own restore done), whether a host registered a handle of its own, and whether every mob of the host is known

### Scenarios
- `mob_hosted_set_census_owed` — mob_hosted_set_lifecycle_machine.rs::whether_a_census_is_owed_is_said_before_the_first_pass says and unsays that the host has stores from before while no pass was begun, keeps every mob known exactly when none can still turn up, and refuses the word once a pass was begun
- `mob_hosted_set_pass` — mob_hosted_set_lifecycle_machine.rs::a_pass_is_begun_over_stores_from_before_and_finishes_once names the first pass once, begins a pass again after one that did not finish, refuses a pass where no census is owed and after one finished, completes the census when a pass finishes, and refuses a finish with no pass begun
- `mob_hosted_set_host_restores_itself` — mob_hosted_set_lifecycle_machine.rs::a_host_that_inserts_its_own_handles_completes_the_set_by_its_word notes a host's handle in every phase and never forgets it, keeps the set incomplete from then until the census is complete, completes the census by the host's declaration in every phase, and refuses a second declaration
