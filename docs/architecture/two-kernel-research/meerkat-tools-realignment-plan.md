# Meerkat Tools Realignment Plan

Status: active alignment plan

This note translates the upstream drift recorded in
`meerkat-upstream-tool-alignment.md` into concrete machine work.

The goal is not to merge code mechanically. The goal is to keep
`MeerkatMachine` honest while the `tools` region is moving upstream.

## Why This Exists

The rebase probe showed that `origin/main` has materially advanced the same
ownership seam we were already expecting to revisit:

- durable tool visibility state
- exact catalog support
- control-plane vs session-plane distinction
- explicit session-task visibility mutation

That means the next successful rebase depends on re-aligning the
`MeerkatMachine.tools` region before trying to replay the old machine branch
blindly.

The concrete upstream baseline for this work is now captured in:

- `meerkat-tool-visibility-upstream-baseline.md`

## Baseline Read

### Current branch exact-current freeze

The current branch exact-current freeze for `K5` says:

- tool-surface truth is the published router authority snapshot
- `ToolScope` is a live bridge over staged/apply visibility behavior
- durable tool visibility ownership is not yet part of the exact-current
  machine boundary

That is still honest for the current branch.

### Upstream implementation read

`origin/main` now clearly contains:

- `SessionToolVisibilityState`
- `ToolVisibilityWitness`
- exact catalog support in dispatcher / gateway / MCP adapter paths
- session-task `SetToolVisibilityState` plumbing

This is no longer just a router-snapshot story.

The detailed upstream ownership read lives in:

- `meerkat-tool-visibility-upstream-baseline.md`

## Required Realignment

### A. Exact-current Meerkat freeze must split the tools region

The current single `K5` asset is too coarse for the upstream baseline.

It should be split conceptually into:

1. **tool visibility ownership**
   - durable visibility state
   - visibility witnesses
   - staged/active revision ownership
   - session-task mutation seam

2. **external tool-surface lifecycle**
   - router authority snapshot
   - staged add/remove/reload
   - removal timing / inflight-call shape

The exact-current freeze should still describe both, but no longer pretend the
entire `tools` region is just router lifecycle.

### B. Target-state Meerkat freeze likely needs a richer `tools` region

The current target `MeerkatMachine.tools` region says:

- known surfaces
- visible surfaces
- staged intents
- pending mutation lineage
- inflight call counts
- published snapshot alignment

That is likely too small now.

The target region probably needs to make explicit:

- durable visibility owner state
- control-plane vs session-plane split
- exact catalog capability / availability
- committed revision used by both provider schema and dispatch gating
- dormant missing intent
- visibility witnesses as ownership proof

Whether all of those become first-class state in the TLA model or some remain
derived predicates should be decided during the realignment pass.

### C. Input/effect alphabet must be updated

The existing Meerkat alphabet should be checked for new tool-region inputs /
effects, likely including some form of:

- stage / replace tool visibility state
- apply staged visibility revision
- publish committed visible set
- catalog refresh / exactness downgrade

The exact names may change, but the machine should stop relying on implicit
tool-publication behavior.

## Recommended Sequence

1. **Absorb upstream code semantics first**
   - `tool_scope.rs`
   - `gateway.rs`
   - `adapter.rs`
   - `ephemeral.rs`

2. **Rewrite exact-current tools freeze**
   - preserve honesty about current branch vs upstream reality
   - split visibility ownership from router lifecycle

3. **Revisit target `MeerkatMachine.tools`**
   - decide which of the new visibility/catalog facts are target machine state
   - update target schema / transitions / obligations if needed

4. **Update TLA scaffold**
   - only after the target tools region is explicit again

5. **Retry rebase**
   - now with machine ownership as the source of truth

## Rebase Success Criteria

The next full rebase attempt counts as successful only if:

- `MeerkatMachine.tools` is re-frozen honestly against the rebased baseline
- the target freeze package is internally consistent after any tool-region
  changes
- Meerkat proof lanes rerun cleanly
- Mob and seam proof packages remain intact

## What Not To Do

- do not treat the current upstream tool changes as generic merge noise
- do not preserve the old exact-current `K5` language if it stops being honest
- do not update the target machine by inference alone; the exact-current tools
  seam needs to be understood first
