# MobLifecycleMachine Mapping Note

This note maps the normative `0.5` `MobLifecycleMachine` contract onto current
`0.4` anchors.

## Rust anchors

- lifecycle enum and command algebra:
  - `meerkat-mob/src/runtime/state.rs`
- serialized owner:
  - `meerkat-mob/src/runtime/actor.rs`
- public surface:
  - `meerkat-mob/src/runtime/handle.rs`

## What is already aligned

- `MobState` already exists with `Creating`, `Running`, `Stopped`,
  `Completed`, `Destroyed`
- lifecycle commands are already serialized by `MobActor`
- stop/resume/complete/destroy/reset already exist as first-class commands
- flow trackers already exist as actor-owned lifecycle-adjacent resources

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- roster membership details
- task-board contents
- wiring graphs
- per-run flow engine internals
- MCP server identity/detail
- surface reply channels

Those refine the same lifecycle owner but are not themselves the lifecycle
contract.

## Important `0.5` clarification

The model keeps `MobLifecycleMachine` intentionally narrow.

That is by design:

- lifecycle transitions stay machine-shaped
- roster/wiring becomes `MobTopologyService`
- task mutation becomes `MobTaskBoardService`
- flow orchestration becomes `FlowRunMachine`

## Known `0.4` divergence

- `MobActor` still centralizes multiple concerns that `0.5` splits apart
- some cleanup/cancellation behavior is currently intertwined with actor logic
  rather than expressed as a standalone lifecycle owner

