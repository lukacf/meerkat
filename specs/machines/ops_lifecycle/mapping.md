# OpsLifecycleMachine Mapping Note

This note maps the normative `0.5` `OpsLifecycleMachine` contract onto current
implementation anchors.

Companion seam spec:

- `docs/architecture/0.5/meerkat_ops_lifecycle_seam_spec.md`

## Rust anchors

- core operation vocabulary:
  - `meerkat-core/src/ops.rs`
- current subagent lifecycle owner to delete:
  - `meerkat-core/src/sub_agent.rs`
- current mob-member lifecycle/orchestration anchors:
  - `meerkat-mob/src/runtime/actor.rs`
  - `meerkat-mob/src/runtime/provisioner.rs`
  - `meerkat-mob/src/roster.rs`
- current background async tool owner to converge:
  - `meerkat-tools/src/builtin/shell/job_manager.rs`

## What is already aligned

- operation IDs, operation results, and `OpEvent` vocabulary already exist
- mob member spawning is already explicitly two-phase
- background async tooling already has real watcher/progress/terminality needs
- mob membership is already event-first with roster projection

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- prompts, budgets, tool policies, and other spawn/tool policy payloads
- actual background task handles
- transcript and UI projections
- roster payload details
- comms/trust setup details
- shell PID/process-group/stdout buffering details

Those are façades or shell details layered on top of the same async-operation
lifecycle semantics.

## Intentional `0.5` shift

The normative machine is a target convergence point, not a claim that current
code already has a single concrete owner with this name.

Specifically:

- there is no surviving lightweight subagent mechanism in `0.5`
- child-agent UX routes through the mob control plane only
- `OpsLifecycleMachine` owns async-operation truth for mob-backed child work and
  background tool operations
- shell jobs refine `BackgroundToolOp` in tool/runtime code; they are not a
  separate machine kind
- `peer_ready(...)` is the lifecycle handoff point into `PeerCommsMachine`

The seam spec names the final concrete owner and trait:

- contracts in `meerkat-core/src/ops_lifecycle.rs`
- authoritative owner in `meerkat-runtime/src/ops_lifecycle.rs`
- mob adaptation in `meerkat-mob/src/runtime/ops_adapter.rs`

## Known precursor divergences

- `SubAgentManager` still owns authoritative child lifecycle today
- mob-member lifecycle is still split across actor, provisioner, and roster
  projection state
- shell-local job registries still own progress, terminality, or watcher truth
- child completion still leaks through comms/transcript/tool-result projection
  instead of remaining purely typed lifecycle output
