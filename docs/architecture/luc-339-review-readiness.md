# LUC-339 Review Readiness Packet

Issue: `LUC-339` fail closed Web SDK mob decoder defaults.

This packet applies to the PR head that contains this file. If the PR head
changes, this packet must be re-read against that exact head before review.

## Scope Separation

Generated:
- `sdks/web/src/generated/mob.ts` adds browser-local mob wire result types from
  `artifacts/schemas/wire-types.json`.

Non-test implementation:
- `sdks/web/src/mob.ts` replaces mob status/result/event casts and fabricated
  fallback projections with strict generated-envelope parsers.
- `sdks/web/src/types.ts` moves public mob status/event shapes to generated
  `status` and `payload` truth, keeping only `state: status` as an inert
  compatibility projection.
- `meerkat-web-runtime/src/lib.rs` emits generated `status` and respawn
  `receipt.member_ref` fields so the real browser WASM path supplies the wire
  truth now required by the SDK.
- `tools/sdk-codegen/generate.py` emits the Web mob generated type slice.

Tests:
- `sdks/web/tests/mob_payload.unit.mjs` adds negative fixtures for missing mob
  status/result fields, legacy respawn receipt carriers, malformed member and
  mob-wide event envelopes, and malformed mob event log entries.
- `sdks/web/tests/e2e_wasm_runtime.test.mjs` updates the fake WASM fixture to
  the generated mob status and respawn receipt shape.

Docs:
- This packet.

## Old Path Amputation Proof

The following exact Web SDK default/fabrication paths became impossible:

- `Mob.status()` no longer returns a raw cast and can no longer accept a
  state-only payload. Missing `status` now throws
  `Invalid mob/status response: missing status`.
- `Mob.memberStatus()` no longer fabricates `status: "unknown"`,
  `tokens_used: 0`, or `is_final: false`. The generated member status,
  token count, and finality flag are all required and type checked.
- `Mob.respawn()` no longer maps missing or unknown result status to
  `completed`, and no longer accepts the legacy `receipt.agent_identity`
  carrier. It requires generated `status`, `receipt.identity`, and
  `receipt.member_ref`.
- `Mob.appendSystemContext()` no longer maps a missing result status to
  `staged`.
- Member and mob-wide subscription decoders no longer synthesize
  `{ type: "unknown" }`, empty `source`, or empty `role` from malformed event
  envelopes. They require generated `payload.type`, mob-wide `source`, and
  mob-wide `role`.
- `Mob.events()` no longer returns a raw cast of malformed event log rows; it
  requires `cursor`, `timestamp`, `mob_id`, and `kind`.

The only retained status compatibility helper is `Mob.status().state`, which is
an inert projection of the already-required generated `status` field.

## Validation Evidence

- Baseline before edits: `npm test` in `sdks/web` passed, and a one-off decoder
  probe showed the old path fabricating `unknown`, `completed`, and
  `{ type: "unknown" }`.
- Focused after edits: `npm test` in `sdks/web` passed.
- Formatting: `make fmt-check` passed.
- Broad check: `MEERKAT_BUILDBUDDY=1 make check` passed via BuildBuddy
  invocation `46c526e1-9a54-4d2b-aa19-e22f94ab1b8d`.
