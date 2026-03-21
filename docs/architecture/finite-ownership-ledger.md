# Finite Ownership Ledger

**Status**: Draft  
**Date**: 2026-03-21

## Purpose

This document turns semantic-ownership drift from an open-ended bug hunt into a
closed-world ledger.

The rule is:

> If a piece of semantically meaningful state or a semantically meaningful
> operation is not classified here and anchored to machine/composition
> ownership, it is not allowed to exist.

This is stricter than seam closure.

- Seam closure asks: "are machine effects that cross owner boundaries
  protocolized?"
- Ownership closure asks: "where does semantic truth live, what operations can
  change it, and are all cross-table couplings modeled?"

The recent recycle and unregister regressions show that seam closure was
necessary but not sufficient: we still have shell-owned semantic orchestration
and shell-owned coupled state.

## Ledger Schema

Every mutable state cell must be classified as exactly one of:

| Class | Meaning | Allowed? |
| --- | --- | --- |
| `machine-owned` | Canonical semantic truth lives in a machine/composition authority | Yes |
| `derived-projection` | Cached/public projection fully derivable from canonical truth | Yes, but must declare its source |
| `capability-handle` | Opaque resource handle with no independent semantic truth (task handle, channel sender, connection handle) | Yes |
| `transport-buffer` | In-flight queue/channel buffering, semantically inert unless coupled invariants say otherwise | Yes |
| `unowned-semantic` | Semantic truth lives here but is not modeled or derivable from modeled truth | No |

Every semantic operation must record:

- owner shell / struct
- inputs
- writeset
- modeled authority/composition anchor
- required postconditions
- coupled invariants it must preserve

Every keyed companion store must declare its coupling invariant explicitly.

Examples:

- `keys(comms_drain_slots) subset keys(sessions)`
- `tool_to_server` matches `servers[*].tools`
- `pending_servers` corresponds to authority-pending surfaces
- `runtime_sessions` corresponds to registered runtime-backed sessions

## Initial Scope Read

This does **not** look like windmills.

The current problem surface appears concentrated in a small number of shell-heavy
structs:

| Subsystem | Primary shell owners | Initial scope feel |
| --- | --- | --- |
| Runtime / session control | `RuntimeSessionAdapter`, `RuntimeSessionEntry`, `CommsDrainSlot` | Medium, but high-risk |
| MCP / external surface | `McpRouter`, `ServerEntry`, `PendingState` | Medium |
| Mob / orchestration | `MobActor`, `SessionBackend`, `MobOpsAdapter`, `RuntimeSessionState` | Large and highest-risk |

The dominant ownership risk is not spread across the whole repo. It clusters in
these shells plus a few helper caches.

## Repo-Wide Scope Snapshot

This first ledger pass is already enough to answer the "windmills or finite?"
question.

The problem is **finite**. The currently visible ownership gap is concentrated
in three handwritten shell clusters plus a few public convenience wrappers.

| Subsystem | Audited mutable state cells | Audited semantic operations | Coupling invariants | Known unowned truth classes | Scope read |
| --- | ---: | ---: | ---: | ---: | --- |
| Runtime / session / control plane | about 22 | 13 | 9 | 5 | Finite, smallest closure project, high regression risk |
| MCP / external surface | about 28 | 15 | 5 | 7 | Finite, mostly one shell owner plus adapter projections |
| Mob / orchestration / helpers | at least 16 handwritten truth cells plus several actor-side companion stores | 8 major composite verbs audited in first pass | 7 | 6 | Largest and riskiest, but still clustered rather than repo-wide |

What this means in practice:

- We are **not** trying to model "everything in the repo."
- We are trying to close semantic ownership in a bounded number of
  shell-dominated subsystems.
- The current gap is not infinite bug surface; it is a finite inventory of
  shell-owned semantic choreography, companion keyed stores, and derived
  projections without explicit derivation contracts.

## Initial Ledger Draft

### Runtime / Session Control

Primary files:

- `meerkat-runtime/src/session_adapter.rs`
- `meerkat-runtime/src/traits.rs`
- `meerkat-machine-schema/src/catalog/runtime_control.rs`

Initial mutable state cells:

- `RuntimeSessionAdapter.sessions`
- `RuntimeSessionAdapter.comms_drain_slots`
- `RuntimeSessionEntry.driver`
- `RuntimeSessionEntry.ops_lifecycle`
- `RuntimeSessionEntry.completions`
- `RuntimeSessionEntry.wake_tx`
- `RuntimeSessionEntry.control_tx`
- `RuntimeSessionEntry._loop_handle`
- `CommsDrainSlot.authority`
- `CommsDrainSlot.handle`
- `CommsDrainSlot.control`

Initial semantic operations:

- `register_session`
- `set_session_silent_intents`
- `register_session_with_executor`
- `ensure_session_with_executor`
- `unregister_session`
- `interrupt_current_run`
- `stop_runtime_executor`
- `accept_input_and_run`
- `accept_input_with_completion`
- `maybe_spawn_comms_drain`
- `notify_comms_drain_exited`
- `abort_comms_drains`
- `abort_comms_drain`
- `wait_comms_drain`
- control-plane verbs `retire`, `recycle`, `reset`, `recover`, `destroy`

Current high-risk couplings:

- `sessions` <-> `comms_drain_slots`
- runtime control-plane recycle semantics <-> driver queue preservation
- executor attachment state <-> `wake_tx` / `control_tx` / loop handle presence
- completion waiters <-> input terminalization

Known unowned semantic truth today:

- recycle semantics are still owner-defined choreography rather than fully
  machine/composition-owned
- session unregister lifetime is not composition-coupled to comms drain slot
  lifetime
- queue truth still lives materially in `InputQueue` / `steer_queue` and is only
  partially projected into machine-owned runtime state
- `CompletionRegistry` is semantically meaningful completion truth, not just
  plumbing, but it has no machine/composition ownership anchor
- registration / executor-attachment / detachment lifetime is still a shell
  semantic composite rather than a machine-owned lifecycle

### MCP / External Surface

Primary files:

- `meerkat-mcp/src/router.rs`
- `meerkat-mcp/src/external_tool_surface_authority.rs`
- `meerkat-machine-schema/src/catalog/external_tool_surface.rs`

Initial mutable state cells:

- `McpRouter.authority`
- `McpRouter.servers`
- `McpRouter.tool_to_server`
- `McpRouter.cached_tools`
- `McpRouter.staged_ops`
- `McpRouter.pending_rx`
- `McpRouter.pending_servers`
- `McpRouter.next_generation`
- `McpRouter.next_boundary_turn`
- `McpRouter.completed_updates`
- `ServerEntry.connection`
- `ServerEntry.tools`
- `ServerEntry.active_calls`
- `PendingState.generation`
- `PendingState.obligation`

Initial semantic operations:

- `add_server`
- `stage_add`
- `stage_remove`
- `stage_reload`
- `apply_staged`
- `execute_effects`
- `spawn_pending`
- `drain_pending`
- `process_pending_result`
- `take_lifecycle_actions`
- `take_external_updates`
- `install_active_server`
- `process_removals`
- `call_tool`
- `shutdown`

Current high-risk couplings:

- authority visible/removing/pending surface state <-> `servers`
- `servers` <-> `tool_to_server`
- authority visible surfaces <-> `cached_tools`
- `pending_servers` <-> pending background tasks/results
- authority inflight call count <-> `ServerEntry.active_calls`
- `completed_updates` <-> delivered external lifecycle notices

Known unowned semantic truth today:

- `tool_to_server` ownership is still shell-maintained and only indirectly
  checked against authority state
- `cached_tools` is shell-built from authority + server entries and has no
  explicit invariant ledger yet
- background pending result generation handling is shell semantics, not a
  modeled coupled lifetime
- the synchronous `install_active_server` compatibility path still defines
  shell semantics outside the staged/boundary lifecycle
- staged-intent ordering, dedupe, and cancellation semantics live in `staged_ops`
  rather than in machine-owned lifecycle truth
- pending task identity (`generation`) and stale-result rejection are shell-owned
- inflight call truth is duplicated between authority `inflight_calls` and
  `ServerEntry.active_calls`
- adapter-side `cached_tools` / `has_pending` are public-surface projections
  without an explicit derivation contract ledger

### Mob / Orchestration

Primary files:

- `meerkat-mob/src/runtime/actor.rs`
- `meerkat-mob/src/runtime/provisioner.rs`
- `meerkat-mob/src/runtime/ops_adapter.rs`
- `meerkat-mob/src/runtime/state.rs`

Initial mutable state cells (non-exhaustive first pass):

- `MobActor.roster`
- `MobActor.task_board`
- `MobActor.state`
- `MobActor.orchestrator`
- `MobActor.run_tasks`
- `MobActor.run_cancel_tokens`
- `MobActor.flow_streams`
- `MobActor.mcp_servers`
- `MobActor.tool_bundles`
- `MobActor.retired_event_index`
- `MobActor.autonomous_host_loops`
- `MobActor.next_spawn_ticket`
- `MobActor.pending_spawns`
- `MobActor.pending_spawn_tasks`
- `MobActor.lifecycle_tasks`
- `MobActor.lifecycle_authority`
- `SessionBackend.runtime_sessions`
- `MobOpsAdapter.operation_ids`
- `RuntimeSessionState.queued_turns`

Initial semantic operations (representative, not exhaustive):

- spawn / spawn-provisioned paths
- retire / retire-all / respawn
- wire / unwire
- external turn / internal turn dispatch
- flow run / flow cancel / flow cleanup
- stop / resume / complete / destroy / reset
- pending spawn failure and rollback paths
- runtime-backed session execution / queue clearing / interrupt / archive

Current high-risk couplings:

- roster <-> orchestrator pending spawn count
- roster <-> pending spawn tables
- roster <-> autonomous host loop handles
- roster <-> retired event index
- run tasks <-> run cancel tokens <-> run store truth
- session-backed provisioner runtime session cache <-> runtime adapter
- member session ids <-> ops adapter operation ids

Known unowned semantic truth today:

- helper wrapper semantics still span multiple shell-managed stores
- runtime-backed session provisioning uses caches whose lifetime coupling is
  only partially modeled
- pending spawn/task ownership is split between authority truth and actor-owned
  BTreeMaps
- respawn is shell-owned semantic choreography, not machine-owned atomic
  lifecycle truth
- wiring truth is split between roster/event projection and live comms-side
  mutation/rollback/notification logic
- auto-spawn-on-external-turn plus deferred delivery is a shell semantic path
  outside a machine-owned operation

## Current Blind Spot Classes

These are the ownership-hole classes the seam pass did not eliminate:

1. **Shell-owned semantic choreography**
   A semantic operation is defined by sequencing multiple legal APIs in owner
   code.

2. **Companion keyed stores without declared coupling invariants**
   Multiple maps keyed by the same identity must stay in sync, but no machine or
   ledger rule states the invariant.

3. **Derived projections without explicit derivation contracts**
   Public caches are intended to be derivable from canonical truth, but the repo
   has no ownership record saying what they derive from and when they must be
   rebuilt.

4. **Compatibility paths that bypass modeled semantics**
   Backward-compatible or convenience paths preserve old behavior outside the
   stronger modeled flow.

## What Would Make This Finite

To close this, we need new repo-wide artifacts and gates:

- a **semantic state inventory** for owner shells
- an **operation ownership map** for all semantic verbs
- a **coupling invariant registry** for companion keyed stores
- RMAT rules that fail on unclassified semantic state or unanchored semantic
  operations

The immediate goal is not to model the entire universe. It is to force the
unmodeled portion into a finite ledger that can be reviewed and burned down.

## Finite Closure Projects

Once the first-pass inventory is grouped by "what would actually need to
change," the repo-wide gap collapses into a finite set of closure projects.

### Runtime / Session

1. **Registration and attachment lifetime model**
   Cover `register_session`, executor attach/upgrade, detach/unregister, and
   destruction so session presence, loop presence, and drain presence cannot
   drift independently.
2. **Session to comms-drain coupling**
   Model `keys(comms_drain_slots) subset keys(sessions)` and its stronger
   lifetime form: no live drain for an unregistered session.
3. **Recycle contract**
   Make recycle semantics machine-owned, including preservation rules for
   queued/staged/recoverable work across driver kind.
4. **Queue truth ownership**
   Decide whether queued input truth stays in driver-local queue structures or
   moves upward into machine-owned state, then ledger every operation against
   that choice.
5. **Completion waiter ownership**
   Either demote `CompletionRegistry` to pure plumbing or model it as semantic
   truth coupled to input terminalization.

### MCP / External Surface

1. **Staged-intent ownership**
   Move add/remove/reload intent ordering and cancellation out of shell-only
   `staged_ops` semantics.
2. **Pending-task identity and task ledger**
   Give connection/enumeration tasks an owned identity/lifetime model rather
   than `generation` plus an untracked background task.
3. **Routing/cache derivation contract**
   Declare and enforce how `servers`, `tool_to_server`, router `cached_tools`,
   and adapter `cached_tools` derive from authority truth.
4. **Inflight-call truth unification**
   Eliminate or formally couple the duplicated authority/call-guard counts.
5. **Compatibility-path closure**
   Remove or fully ledger the `install_active_server` and adapter-side
   compatibility semantics.

### Mob / Orchestration / Helpers

1. **Member lifecycle machine ownership**
   Spawn finalization, attach/resume, retire/disposal, and respawn need
   machine-owned atomic lifecycle semantics instead of actor choreography.
2. **Pending spawn / rollback ledger**
   Couple `pending_spawns`, `pending_spawn_tasks`, orchestrator pending counts,
   roster commit, and rollback paths.
3. **Runtime-backed session bridge**
   Close `runtime_sessions`, `queued_turns`, runtime adapter registration, and
   `MobOpsAdapter.operation_ids` into one owned bridge model.
4. **Helper wrapper contract**
   Move `spawn_helper`, `fork_helper`, `respawn`, and member wait helpers from
   best-effort convenience choreography to explicit derived contract surfaces.
5. **Wiring truth closure**
   Couple roster wiring, trust-edge mutation, notifications, event projection,
   rollback, and edge-lock cleanup.
6. **Auto-spawn external turn semantics**
   Either machine-own or explicitly outlaw the current shell choreography that
   mixes target resolution, spawn, and deferred delivery.

## Current Verdict

This is **manageable**.

Why:

- the unowned semantic truth is concentrated in three shell-heavy subsystems,
  not spread uniformly across the repo
- each subsystem collapses into a small number of closure projects
- the scary part is not cardinality, it is coupling density inside those shells

Why it still hurts:

- the mob shell is large and mixes orchestration, lifecycle, provisioning,
  comms, and convenience wrappers
- the runtime/session shell has fewer moving pieces but directly affects
  correctness and recovery
- MCP has a good machine core already, but still carries a second shell-owned
  semantics path in routing, pending task identity, and compatibility APIs

The right takeaway is:

> We are not tilting at windmills, but we also are not one audit away from
> closure. The problem is finite enough to ledger and burn down, and large
> enough that we need to treat semantic ownership as a first-class closure
> project rather than as an extension of seam validation.

## First Closure Tranches

### Tranche A: Runtime / Session ownership closure

Target outcome:

- `recycle`, `register_session`, `unregister_session`, executor attach/detach,
  and comms drain lifetime become fully ledgered and explicitly coupled.

Why first:

- highest current regression risk
- smallest shell surface area
- directly exercises the difference between seam closure and ownership closure

### Tranche B: MCP router ownership closure

Target outcome:

- `servers`, `tool_to_server`, `cached_tools`, pending completion state, and
  compatibility add/reload paths receive explicit derivation/coupling rules.

Why second:

- single shell owner
- clear authority already exists
- good proving ground for derived-projection rules

### Tranche C: Mob shell decomposition

Target outcome:

- turn helper semantics, pending spawn ownership, runtime-backed session caches,
  and roster/run/task couplings get split into explicit ownership units.

Why last:

- this is the largest shell and probably the longest closure effort
- likely requires additional machine/composition extraction, not just audit
