# Meerkat Shadow Validation Plan

Status: active pre-cutover validation plan

This note turns `meerkat-cutover-lowering-inventory.md` into the first explicit
shadow-validation plan for `MeerkatMachine`.

The goal is short-lived validation over the live implementation before any
write-side switch. The machine remains read-only; the implementation still owns
behavior.

## What to shadow

Construct `MeerkatMachine` snapshots from the live runtime at the same
observation points where the implementation already exposes stable state.

Primary comparison surfaces:

- session adapter snapshot
- joined `MeerkatMachine` snapshot
- turn/ops authority state
- peer ingress runtime snapshot
- tool visibility / tool surface snapshot
- drain snapshot

## First shadow lanes

### S1. Lifecycle/control lane

Compare:

- machine control phase
- binding presence
- `current_run_id`
- queue and steer visibility
- completion-waiter teardown

Use for:

- retire/reset/recover/recycle/stop/destroy paths
- plain vs attached runtime divergence

Expected mismatch classes:

- stale helper projection
- lifecycle command lowered through too many helper paths
- hidden current-run truth surviving terminal lifecycle

### S2. Turn / ops / barrier lane

Compare:

- active run presence
- pending op refs
- barrier membership/satisfaction
- `wait_all` identity
- watcher/pending counts

Use for:

- tool-call resolution
- background completion
- barrier satisfaction
- retry / cancellation observation

Expected mismatch classes:

- runner-path helper drift
- stale protocol submission ordering
- barrier truth split away from ops truth

### S3. Peer-ingress lane

Compare:

- trust set
- auth mode
- classified backlog
- admitted peer lineage
- terminalized peer lineage

Use for:

- receive
- trust mutation
- drain
- replay/recovery boundaries

Expected mismatch classes:

- queue truth bypass
- stale replay assumptions
- trust mutation occurring outside canonical comms seam

### S4. Tools lane

Compare:

- durable `tool_visibility` state
- visible projection
- missing requested/filter names
- `tool_surface` staged/pending/applied snapshot
- committed published set alignment

Use for:

- session-task visibility mutation
- MCP stage/apply/finalize
- exact catalog projection

Expected mismatch classes:

- visibility/surface publication split
- stale projection surviving a newer committed revision
- target-only committed visible-set contract not yet implemented

### S5. Drain lane

Compare:

- drain mode
- task phase
- suppression state
- comms binding presence

Use for:

- comms drain spawn/exit/abort
- keep-alive changes
- runtime teardown with drain state present

Expected mismatch classes:

- helper-only drain transitions
- stale published mode after binding loss

## Mismatch triage

Every mismatch should be classified as exactly one of:

- `implementation_detail`
- `semantic_gap`
- `dogma_violation`

### `implementation_detail`

The machine is right, but a helper/protocol/order-of-operations path differs.

### `semantic_gap`

The frozen target machine is missing a fact, transition, or obligation the live
system genuinely needs.

### `dogma_violation`

The mismatch exists because semantic truth is still split across owners,
helpers, shell paths, or bypass seams.

## Exit criteria

This plan is complete enough to start implementation once:

- one concrete shadow-read hook exists for each of the five lanes above
- mismatch output can name the lane, machine region, and triage class
- shadow runs can be exercised on representative lifecycle and tools paths

## Read with

- `meerkat-cutover-lowering-inventory.md`
- `meerkat-machine-refinement-delta.md`
- `two-kernel-refinement-program.md`
