# Meerkat Upstream Tool Alignment

Status: active alignment note

This note records the first honest rebase probe against `origin/main` after the
machine freeze / proof work landed.

The purpose is not to describe merge mechanics. The purpose is to identify
which machine-owned areas have drifted upstream, and what must be realigned
before the next rebase attempt.

## Result Of The Rebase Probe

The first replayed machine commit (`Build Meerkat and Mob machine diagnostic
kernels`) collided immediately in:

- `meerkat-core/src/tool_scope.rs`
- `meerkat-core/src/gateway.rs`
- `meerkat-mcp/src/adapter.rs`
- `meerkat-session/src/ephemeral.rs`
- `meerkat-core/src/lib.rs`
- `examples/035-mdm-tux-rs/src/bin/target.rs`

The important result is that the semantic drift is concentrated in the
`MeerkatMachine.tools` seam, not spread uniformly across Meerkat or Mob.

## Ownership Read

`origin/main` has already moved materially toward:

- durable session-owned tool visibility state
- exact catalog capabilities and catalog projection
- control-plane vs session-plane tool separation
- session-task control seams for tool visibility mutation

That means the current branch and `origin/main` are no longer merely in a
"same behavior, different patch shape" situation. They are converging on the
same target ownership problem from different directions.

## Machine Impact

### MeerkatMachine

This drift primarily reopens the `tools` region of the frozen
`MeerkatMachine` package.

The upstream changes are strongly aligned with a more explicit machine-owned
tool visibility domain:

- `ToolScope` is no longer just a simple staged filter owner
- durable state and visibility witnesses exist
- exact catalog support is entering the dispatcher / gateway path
- the live session task is gaining an explicit tool-visibility mutation seam

That means the next correct move is not to force the old freeze across the
rebase, but to realign the `tools` region with the upstream ownership shift and
then re-freeze it.

### MobMachine

No primary `MobMachine` ownership area appears to have drifted in the first
conflict wave.

The rebase probe did not indicate comparable semantic drift in:

- flow kernel ownership
- work ledger
- authority / fencing
- topology intent
- frame / loop structure

So Mob stays downstream of this alignment work.

## Conflict Classification

### High semantic drift

These files should be treated as ownership alignment work, not conflict
resolution work:

- `meerkat-core/src/tool_scope.rs`
- `meerkat-core/src/gateway.rs`
- `meerkat-mcp/src/adapter.rs`
- `meerkat-session/src/ephemeral.rs`

### Mostly mechanical drift

These files appear to be export / example fallout from the same seam:

- `meerkat-core/src/lib.rs`
- `examples/035-mdm-tux-rs/src/bin/target.rs`

## Alignment Plan

Before the next full rebase attempt:

1. absorb `origin/main` tool visibility / catalog / session-control changes as
   the new implementation baseline for the `tools` region
2. rewrite the `MeerkatMachine.tools` freeze notes against that baseline
3. update the target-state `MeerkatMachine` freeze and TLA scaffold if the
   upstream ownership move changes the target machine, not just the
   exact-current baseline
4. re-run the Meerkat freeze / package audits
5. then retry the full rebase and proof lanes

## What This Note Is Not Claiming

This note does not claim that the existing Meerkat freeze is invalid.

It claims something narrower and more useful:

- the current branch freeze still describes the current branch honestly
- `origin/main` has moved the `tools` seam enough that the next rebase should
  be machine-led, not conflict-led

That is a normal consequence of doing real ownership work in parallel.
