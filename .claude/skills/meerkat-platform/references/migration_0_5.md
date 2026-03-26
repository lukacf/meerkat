# Meerkat 0.4x -> 0.5 Migration Guide

This guide is the fastest way to realign older Meerkat integrations and mental models with the settled 0.5 contract.

## Big mental-model changes

### Runtime-backed surfaces are canonical

In 0.5, the product semantics live in the runtime-backed path:

- runtime ingress owns Queue/Steer routing
- runtime owns `keep_alive`
- runtime owns comms drain lifecycle
- runtime owns request cancellation and commit-boundary truth

`SessionService` still matters, but as substrate. It is not the primary semantic owner of those runtime facts.

### `EphemeralSessionService` still exists, but it is not the main product path

Use it for:

- tests
- embedded/in-memory use
- WASM internals
- narrow Queue-only execution

Do not expect it to own:

- `keep_alive`
- Steer/render-metadata semantics
- runtime-backed ingress behavior

### Mob is the multi-agent path

There is no separate sub-agent/helper-agent architecture outside mob orchestration.

If older material talks about helper agents, forks, delegates, or autonomous assistants as a separate system, translate that into the mob runtime.

## Terminology changes

| 0.4x term | 0.5 term |
|---|---|
| `host_mode` | `keep_alive` |
| `host mode` | keep-alive / runtime-backed session behavior |
| `--host` | `--keep-alive` |

Meaning:

- session remains alive after the turn completes
- future admitted work becomes future turns
- this is a runtime/session behavior, not an old “host loop” execution model

## Behavioral changes

### External events are queue-only runtime inputs

Older intuition:

- external events “wake immediately”
- direct inbox draining is the main mental model

0.5 contract:

- external events are queue-only runtime-backed inputs
- they are admitted through runtime ingress
- committed work is not reinterpreted by a second shadow path

### Request cancellation only affects uncommitted work

If cancel lands before the operation commits:

- return cancelled
- side effects should be rolled back or prevented

If the operation already committed success:

- success stays success
- do not rewrite it to cancellation

If create already crossed the commit boundary and the first turn failed:

- surface session identity
- keep the session resumable

### `keep_alive` semantics are now explicit

- create/run omitted => default `false`
- continue/resume omitted => inherit persisted session intent
- explicit override commits as a session/runtime mutation once validated

## API and wire-shape changes

### Presence matters now

In several places, omission vs explicit false/default now matters.

Typical migration:

- old: plain bool/u32 with “default means inherit” folklore
- new: `Option<bool>` / `Option<u32>`-style semantics where omission really means “no explicit override”

This especially affects:

- `keep_alive`
- MCP builtins/shell toggles
- structured output retry counts

### Rust deferred first-turn `system_prompt`

Rust now supports `system_prompt` on `StartTurnRequest`, but only for a deferred session’s first turn.

Allowed:

- create deferred session
- first `start_turn()` supplies `system_prompt`

Rejected:

- changing `system_prompt` after the session already has history

## Rust embedding migration

### Default guidance changed

If your old embedding story looked like:

```rust
let service = build_ephemeral_service(factory, config, 64);
```

that is now the **substrate/testing** story, not the primary production recommendation.

For the main production path, prefer runtime-backed persistent construction via:

- `open_realm_persistence_in(...)`
- `build_persistent_service(...)`

### When `build_ephemeral_service` is still valid

Use it when you intentionally want:

- in-memory sessions
- Queue-only execution
- no runtime-backed `keep_alive`
- no Steer/render-metadata semantics

If you expect runtime behavior from it, that is the migration bug.

## Common breakages and fixes

### “My old `host_mode` field no longer works”

Replace it with `keep_alive`.

Also update CLI/docs/examples:

- `--host` -> `--keep-alive`

### “I changed a resume field and it got ignored”

0.5 uses explicit override intent and resumed metadata merge rules. If the surface cannot prove explicit presence, it cannot reliably override persisted state.

### “External events suddenly feel less immediate”

That is usually the queue-only runtime-backed external-event contract. Update your mental model from “direct inbox drain” to “runtime admission queue.”

### “Cancelled requests still changed state”

Only uncommitted work should be cancelled away. If a create or turn already committed, clients should treat the returned identity/success as canonical.

## Surface-by-surface checklist

### CLI

- replace `--host` with `--keep-alive`
- update examples/scripts that assume old host-mode wording

### REST

- create: omitted `keep_alive` defaults to `false`
- continue: omitted `keep_alive` inherits persisted intent
- external events are queue-only
- use request IDs / cancel endpoint with the committed-vs-uncommitted model

### RPC

- same runtime-backed commit/cancel model
- live hot-swap remains surface-specific where documented

### MCP

- `meerkat_run` create semantics follow the same commit-boundary rules
- `meerkat_resume` is a real resume/rebuild-capable surface, not just “turn again”

### Python SDK

- presenceful optional fields matter
- do not rely on truthy/non-default folklore for omitted vs explicit overrides

### TypeScript SDK

- same presenceful semantics as Python
- prefer omission for inherit; send explicit values when you mean override

### Rust SDK

- use runtime-backed persistent flow as the default story
- treat `build_ephemeral_service` as a narrower substrate API
- use deferred first-turn `system_prompt` correctly
