# LUC-339 Review Readiness Packet

> **Current status (0.6.5):** Historical review packet for a specific Web SDK
> PR head. The generated mob envelope and fail-closed decoder principles remain
> relevant, but fields such as `realtime_attachment_status` below are preserved
> as review-context vocabulary from that PR, not as current public live-channel
> API guidance.

Issue: `LUC-339` fail closed Web SDK mob decoder defaults.

This packet applies to the PR head that contains this file. If the PR head
changes, this packet must be re-read against that exact head before review.

## Scope Separation

Generated:
- `sdks/web/src/generated/mob.ts` adds browser-local mob wire result types from
  `artifacts/schemas/wire-types.json`, including generated `MobListResult`,
  `MobMemberSendResult`, `MobFlowStatusResult`, `MobHelperResult`,
  `MobMemberStatusResult`, and supporting `WireMemberRef` contracts.

Non-test implementation:
- `sdks/web/src/mob.ts` replaces mob status/result/event casts and fabricated
  fallback projections with strict generated-envelope parsers. The mob status
  parser is shared with runtime mob listing.
- `sdks/web/src/mob.ts` routes `Member.send()`, `spawnHelper()`,
  `forkHelper()`, and `flowStatus()` through generated/fail-closed result
  parsers instead of raw casts or caller/default projections.
- `sdks/web/src/mob.ts` requires canonical event envelope metadata
  (`event_id`, `source_id`, `seq`, `timestamp_ms`) before exposing member or
  mob-wide subscription events.
- `sdks/web/src/runtime.ts` decodes `MeerkatRuntime.listMobs()` as a generated
  `MobListResult` envelope and validates every mob row with the generated mob
  status parser.
- `sdks/web/src/types.ts` moves public mob status/event shapes to generated
  `status`, event metadata, and `payload` truth, keeping only `state: status`
  as an inert compatibility projection. `MobMemberSnapshot` now preserves the
  generated member-status fields `current_session_id`,
  `realtime_attachment_status`, and `external_member`.
- `meerkat-web-runtime/src/lib.rs` emits generated `status` and respawn
  `receipt.member_ref` fields, and `mob_list()` now emits the generated
  `MobListResult` envelope. It also emits generated `mob_id` on
  `mob_member_send()` and wraps `mob_flow_status()` as generated
  `{ "run": ... }`, so the real browser WASM path supplies the wire truth now
  required by the SDK.
- `tools/sdk-codegen/generate.py` emits the Web mob generated type slice.
- The branch is rebased onto current `origin/main`; the exact-head two-dot diff
  no longer contains the stale `meerkat-runtime/src/driver/persistent.rs` or
  `meerkat-session/src/persistent.rs` regressions called out in rework.

Tests:
- `sdks/web/tests/mob_payload.unit.mjs` adds negative fixtures for missing mob
  status/result fields, `MeerkatRuntime.listMobs()` state-only/malformed rows,
  legacy respawn receipt carriers, malformed member and mob-wide event
  envelopes, metadata-light `{ payload: { type } }` envelopes, malformed
  helper/send/flow result envelopes, and malformed mob event log entries.
- `sdks/web/tests/e2e_wasm_runtime.test.mjs` updates the fake WASM fixture to
  the generated mob list, member status, member-send, flow-status, and respawn
  receipt shape.

Docs:
- This packet.

## Old Path Amputation Proof

The following exact Web SDK default/fabrication paths became impossible:

- `Mob.status()` no longer returns a raw cast and can no longer accept a
  state-only payload. Missing `status` now throws
  `Invalid mob/status response: missing status`.
- `MeerkatRuntime.listMobs()` no longer returns
  `JSON.parse(json) as MobStatus[]`. It now requires the generated
  `MobListResult.mobs` envelope and validates each row as a generated
  `MobStatusResult`. A state-only row such as `{ mob_id, state }` now throws
  `Invalid mob/list response.mobs[0]: missing status`, and a legacy bare array
  now throws `Invalid mob/list response: malformed envelope`.
- `Mob.memberStatus()` no longer fabricates `status: "unknown"`,
  `tokens_used: 0`, or `is_final: false`. The generated member status,
  token count, and finality flag are all required and type checked. The
  generated `current_session_id`, `realtime_attachment_status`, and
  `external_member` fields are preserved after validation instead of being
  dropped.
- `Member.send()` no longer raw-casts a partial receipt or projects missing
  returned `handling_mode` from the caller request. It requires generated
  `mob_id`, `agent_identity`, `member_ref`, and `handling_mode`; missing
  `handling_mode` now throws
  `Invalid mob member delivery response: missing handling_mode`.
- `Mob.spawnHelper()` and `Mob.forkHelper()` no longer fabricate
  `tokens_used: 0`. They require generated `tokens_used`, `agent_identity`, and
  `member_ref`; missing token truth now throws
  `Invalid mob spawn_helper response: tokens_used must be number` or the
  matching fork-helper error.
- `Mob.flowStatus()` no longer returns `JSON.parse(json) as FlowStatus`. It
  requires the generated `MobFlowStatusResult.run` envelope, returns `null` only
  for explicit `run: null`, and validates `run_id` and `status` before exposing
  a run object. A malformed legacy payload such as `{ state: "running" }` now
  throws `Invalid mob flow_status response: missing run`.
- `Mob.respawn()` no longer maps missing or unknown result status to
  `completed`, and no longer accepts the legacy `receipt.agent_identity`
  carrier. It requires generated `status`, `receipt.identity`, and
  `receipt.member_ref`.
- `Mob.appendSystemContext()` no longer maps a missing result status to
  `staged`.
- Member and mob-wide subscription decoders no longer synthesize
  `{ type: "unknown" }`, empty `source`, or empty `role` from malformed event
  envelopes. They require generated `payload.type`, a validated mob-wide
  runtime-id `source` object, mob-wide `role`, and canonical envelope
  metadata (`event_id`, `source_id`, `seq`, `timestamp_ms`). A metadata-light
  `{ payload: { type } }` envelope now throws
  `Invalid mob member subscription event[0]: missing event_id`.
- `Mob.events()` no longer returns a raw cast of malformed event log rows; it
  requires `cursor`, `timestamp`, `mob_id`, and `kind`.

The only retained status compatibility helper is `state` on public mob status
objects from `Mob.status()` and `MeerkatRuntime.listMobs()`. In both cases it is
an inert projection of the already-required generated `status` field.

## Validation Evidence

- Baseline before edits: `npm test` in `sdks/web` passed, and a one-off decoder
  probe showed the old path fabricating `unknown`, `completed`, and
  `{ type: "unknown" }`.
- Focused after edits: `npm test` in `sdks/web` passed.
- Formatting: `make fmt-check` passed.
- Broad check: `MEERKAT_BUILDBUDDY=1 make check` passed via BuildBuddy
  invocation `46c526e1-9a54-4d2b-aa19-e22f94ab1b8d`.
- AI review follow-up: mob-wide `source` now requires the real runtime-id object
  shape and rejects the legacy string source instead of projecting
  `agent_identity`.
  Revalidated with `npm test` in `sdks/web` and `make agent-gate`.
- Human Review rework baseline at
  `962ad4fdae385ed228d2a324aaf38b0d2edc2677`: `npm test` in `sdks/web` passed,
  and a one-off `MeerkatRuntime.listMobs()` probe showed the old raw-cast path
  returned `[{"mob_id":"mob-web-unit","state":"Running"}]` from a state-only
  `mob_list()` row.
- Rework focused validation: `npm test` in `sdks/web` passed after routing
  `MeerkatRuntime.listMobs()` through the generated mob list/status parsers and
  fixing the stale e2e member-status fixture.
- Second rework baseline at
  `36e3c85f763e5325677cb57fe9ce1ca0f7a306c7`: `npm test` in `sdks/web`
  passed, and one-off probes confirmed the old paths still projected missing
  `handling_mode`, fabricated helper `tokens_used: 0`, raw-cast malformed
  flow-status payloads, and accepted metadata-light member events.
- Second rework focused validation: `npm test` in `sdks/web` passed after
  adding generated parsers for member-send, helper, and flow-status results;
  requiring canonical event envelope metadata; preserving generated
  member-status fields; and rebasing away the stale non-Web exact-head diff.
- Second rework one-off probes now fail closed with
  `Invalid mob member delivery response: missing handling_mode`,
  `Invalid mob spawn_helper response: tokens_used must be number`,
  `Invalid mob fork_helper response: tokens_used must be number`,
  `Invalid mob flow_status response: missing run`, and
  `Invalid mob member subscription event[0]: missing event_id`.
- Second rework targeted Rust compile:
  `./scripts/repo-cargo test -p meerkat-web-runtime --no-run` passed.
- Local Web e2e attempt: `npm run test:e2e` could not start because
  `wasm-pack` is not installed in this workspace (`spawn wasm-pack ENOENT`).
