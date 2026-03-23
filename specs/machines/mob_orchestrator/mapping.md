# MobOrchestratorMachine Mapping Note

This note maps the normative `0.5` `MobOrchestratorMachine` contract onto
current `0.4` anchors.

## Rust anchors

- central owner and command serialization:
  - `meerkat-mob/src/runtime/actor.rs`
- definition-level orchestrator identity:
  - `meerkat-mob/src/definition.rs`
  - `meerkat-mob/src/validate.rs`
- runtime builder/runtime-mode coordination:
  - `meerkat-mob/src/runtime/builder.rs`
  - `meerkat-mob/src/runtime/handle.rs`

## What is already aligned

- current code already has a distinguished orchestrator profile in definition
- actor-owned orchestration already tracks pending spawns, flow tasks, host
  loops, and runtime-mode coordination
- auto-wiring and lifecycle notifications already treat the orchestrator as a
  first-class coordination role

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- concrete roster/member identities
- topology rule contents
- supervisor escalation payloads
- exact spawn tickets and runtime-mode details
- task-board contents
- flow-engine inner behavior

Those refine orchestration ownership but do not change the top-level
coordination contract.

## Important `0.5` clarification

Today, "orchestrator" is more owner-semantics than a clearly named state
machine. The formal `0.5` contract makes that ownership explicit.

This is intentional:

- `MobLifecycleMachine` owns top-level lifecycle phase
- `FlowRunMachine` owns durable per-run semantics
- `MobOrchestratorMachine` owns coordination and authority across them

## Known `0.4` divergence

- current orchestrator identity is derived from definition + roster rather
  than an explicit named orchestration owner
- `MobActor` still centralizes the orchestration concerns this contract makes
  explicit
