# FlowRunMachine Mapping Note

This note maps the normative `0.5` `FlowRunMachine` contract onto current
`0.4` anchors.

## Rust anchors

- durable run aggregate:
  - `meerkat-mob/src/run.rs`
- store and CAS semantics:
  - `meerkat-mob/src/store/mod.rs`
  - `meerkat-mob/src/store/in_memory.rs`
- runtime execution/terminalization:
  - `meerkat-mob/src/runtime/flow.rs`
  - `meerkat-mob/src/runtime/terminalization.rs`

## What is already aligned

- `MobRunStatus` already exists as `Pending`, `Running`, `Completed`,
  `Failed`, `Canceled`
- step ledgers and failure ledgers already exist
- output persistence is already step-ledger-aware
- terminalization is already CAS-guarded and terminal-state-aware

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- full flow topology and condition evaluation
- retry budgets and timeout windows
- branch semantics
- target profile/member selection
- concrete event streams and JSON payloads

Those refine the same run aggregate semantics.

## Important `0.5` clarification

`FlowRunMachine` is the durable run owner. It is not a mob-local turn executor.

Step dispatch may produce runtime/turn work, but that work crosses machine
boundaries and returns through effects/terminalization rather than making flow
execution itself into a second execution loop.

## Known `0.4` divergence

- actor-side task/cancel trackers still coexist with store-owned durable truth
- some fallback terminalization still lives in actor cleanup code rather than
  one obviously named machine owner
