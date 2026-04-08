# MobMemberBootstrapMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_member_bootstrap`

## State
- Phase enum: `Tracking`
- `started_runs`: `Set<RunId>`
- `callback_pending_runs`: `Set<RunId>`
- `failed_runs`: `Set<RunId>`
- `cancelled_runs`: `Set<RunId>`
- `force_cancel_count`: `u32`

## Inputs
- `KickoffStarted`(run_id: RunId)
- `KickoffCallbackPending`(run_id: RunId)
- `KickoffFailed`(run_id: RunId)
- `KickoffCancelled`(run_id: RunId)
- `KickoffForceCancelled`

## Effects
- `BootstrapStateUpdated`

## Invariants
- `started_not_failed`
- `started_not_cancelled`
- `failed_not_cancelled`
- `callback_pending_not_terminal`

## Transitions
### `KickoffStarted`
- From: `Tracking`
- On: `KickoffStarted`(run_id)
- Emits: `BootstrapStateUpdated`
- To: `Tracking`

### `KickoffCallbackPending`
- From: `Tracking`
- On: `KickoffCallbackPending`(run_id)
- Emits: `BootstrapStateUpdated`
- To: `Tracking`

### `KickoffFailed`
- From: `Tracking`
- On: `KickoffFailed`(run_id)
- Emits: `BootstrapStateUpdated`
- To: `Tracking`

### `KickoffCancelled`
- From: `Tracking`
- On: `KickoffCancelled`(run_id)
- Emits: `BootstrapStateUpdated`
- To: `Tracking`

### `KickoffForceCancelled`
- From: `Tracking`
- On: `KickoffForceCancelled`()
- Emits: `BootstrapStateUpdated`
- To: `Tracking`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/handle.rs` — member/member-list surfaces that project durable kickoff state
- `meerkat-mob/src/runtime/actor.rs` — runtime command handling path that realizes kickoff bootstrap transitions
- `meerkat-mob/src/runtime/mob_member_bootstrap_authority.rs` — bootstrap authority that owns kickoff transition legality

### Scenarios
- `kickoff-started` — successful kickoff completion transitions bootstrap state to started
- `kickoff-failed-or-cancelled` — failed and cancelled kickoff outcomes transition bootstrap state and emit typed lifecycle notices
- `kickoff-callback-pending` — callback-pending kickoff remains non-terminal and distinct from member lifecycle corruption
