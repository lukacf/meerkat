# MobMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobMachine`

### Code Anchors
- `mob_handle_surface`: `meerkat-mob/src/runtime/handle.rs` — identity-first public MobMachine handle surface
- `mob_actor_authority`: `meerkat-mob/src/runtime/actor.rs` — MobMachine actor authority and command execution

### Scenarios
- `spawn-work-terminal` — member spawn, runtime-ready observation, work submission, and terminal work closure
- `retire-respawn-destroy` — member retires, respawns with a new runtime incarnation, and destroys cleanly

### Transitions
- `Start`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `SpawnMember`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `ObserveRuntimeReady`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `SubmitWork`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `ObserveWorkCompleted`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `ObserveWorkFailed`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `ObserveWorkCancelled`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `RetireMember`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `ObserveRuntimeRetired`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `ResetMember`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `RespawnMember`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `MarkCompleted`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `DestroyMob`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `ObserveRuntimeDestroyed`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`

### Effects
- `RequestRuntimeBinding`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `SubmitMemberWork`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `RequestRuntimeRetire`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `RequestRuntimeDestroy`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `EmitMemberLifecycleNotice`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`

### Invariants
- `active_work_requires_runtime`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `destroyed_has_no_active_runtime`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`
- `active_runtime_has_identity`
  - anchors: `mob_handle_surface`, `mob_actor_authority`
  - scenarios: `spawn-work-terminal`, `retire-respawn-destroy`


<!-- GENERATED_COVERAGE_END -->
