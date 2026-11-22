# MobHostedSetLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobHostedSetLifecycleMachine`

### Code Anchors
- `mob_hosted_set_lifecycle` (machine `MobHostedSetLifecycleMachine`): `crates/meerkat-mob/src/machines/mob_hosted_set_lifecycle.rs` — MobHostedSetLifecycleMachine owns the facts about a host's mob set as a whole, which no store's record can hold: how far the pass over the stores the host has from before came (not begun, begun and not finished, finished), whether a census of such stores is owed, whether the census is complete (a pass finished, or the host declared its own restore done), whether a host registered a handle of its own, and whether every mob of the host is known

### Scenarios
- `mob_hosted_set_census_owed` — mob_hosted_set_lifecycle_machine.rs::whether_a_census_is_owed_is_said_before_the_first_pass says and unsays that the host has stores from before while no pass was begun, keeps every mob known exactly when none can still turn up, and refuses the word once a pass was begun
- `mob_hosted_set_pass` — mob_hosted_set_lifecycle_machine.rs::a_pass_is_begun_over_stores_from_before_and_finishes_once names the first pass once, begins a pass again after one that did not finish, refuses a pass where no census is owed and after one finished, completes the census when a pass finishes, and refuses a finish with no pass begun
- `mob_hosted_set_host_restores_itself` — mob_hosted_set_lifecycle_machine.rs::a_host_that_inserts_its_own_handles_completes_the_set_by_its_word notes a host's handle in every phase and never forgets it, keeps the set incomplete from then until the census is complete, completes the census by the host's declaration in every phase, and refuses a second declaration

### Transitions
- `ExpectCensusOwed`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`
- `ExpectCensusOwedComplete`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`
- `ExpectCensusNotOwed`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`
- `ExpectCensusNotOwedHostInserts`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`
- `ExpectCensusAttempted`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`
- `ExpectCensusRestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`
- `BeginPassFirst`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `BeginPassNoCensusOwed`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `BeginPassAgain`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `BeginPassRestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `FinishPassCompletesCensus`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `FinishPassCensusComplete`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `FinishPassUnrestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `FinishPassRestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `DeclareRestoredUnrestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `DeclareRestoredUnrestoredComplete`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `DeclareRestoredAttempted`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `DeclareRestoredAttemptedComplete`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `DeclareRestoredRestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `RegisterHostHandleUnrestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `RegisterHostHandleUnrestoredComplete`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `RegisterHostHandleAttempted`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `RegisterHostHandleAttemptedComplete`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `RegisterHostHandleRestored`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`

### Effects
- `CensusExpected`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`
- `PassBegun`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `PassFinished`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `CensusCompleted`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`, `mob_hosted_set_host_restores_itself`
- `HostHandleNoted`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_host_restores_itself`
- `RequestRefused`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`, `mob_hosted_set_pass`, `mob_hosted_set_host_restores_itself`

### Invariants
- `a_restored_set_has_its_census`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `a_pass_is_begun_only_where_a_census_is_owed`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_pass`
- `all_mobs_are_known_exactly_when_none_can_still_turn_up`
  - anchors: `mob_hosted_set_lifecycle`
  - scenarios: `mob_hosted_set_census_owed`, `mob_hosted_set_host_restores_itself`


<!-- GENERATED_COVERAGE_END -->
