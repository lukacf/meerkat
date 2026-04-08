# MobMemberBootstrapMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobMemberBootstrapMachine`

### Code Anchors
- `mob_runtime_handle`: `meerkat-mob/src/runtime/handle.rs` — member/member-list surfaces that project durable kickoff state
- `mob_runtime_actor`: `meerkat-mob/src/runtime/actor.rs` — runtime command handling path that realizes kickoff bootstrap transitions
- `mob_bootstrap_authority`: `meerkat-mob/src/runtime/mob_member_bootstrap_authority.rs` — bootstrap authority that owns kickoff transition legality

### Scenarios
- `kickoff-started` — successful kickoff completion transitions bootstrap state to started
- `kickoff-failed-or-cancelled` — failed and cancelled kickoff outcomes transition bootstrap state and emit typed lifecycle notices
- `kickoff-callback-pending` — callback-pending kickoff remains non-terminal and distinct from member lifecycle corruption

### Transitions
- `KickoffStarted`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`
- `KickoffCallbackPending`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`
- `KickoffFailed`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`
- `KickoffCancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`
- `KickoffForceCancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`

### Effects
- `BootstrapStateUpdated`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`

### Invariants
- `started_not_failed`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`
- `started_not_cancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`
- `failed_not_cancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`
- `callback_pending_not_terminal`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_bootstrap_authority`
  - scenarios: `kickoff-started`


<!-- GENERATED_COVERAGE_END -->
