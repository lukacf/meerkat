# LUC-347 Review Readiness Packet

Issue: LUC-347, P1 Mechanical Slice: typed realtime protocol version wire
surface.
Branch: `codex/LUC-347-typed-realtime-protocol`.

Dogma cleanup PR: yes.

## Review Readiness Packet

The PR body is the authoritative exact-current-head packet consumed by the
Dogma cleanup review gate after each push. This committed packet copy certifies
the implementation/test slice and records the gate-compatible proof shape
without duplicating the PR body's push-specific head SHA.

Command output:

```text
$ git diff --check
<no output>
```

Commit-count explanation: the clean-history implementation/test slice is
exactly 3 commits:

- `cfd5b213acad44652bcd029ffdb19fe66d951cfd` types the realtime protocol
  version wire surface, updates host/SDK behavior, regenerates artifacts, and
  adds the initial packet.
- `78ef35d62eaec681499eed08ac2f05d67514388b` fixes typed realtime protocol
  lint fallout.
- `cf0a86b426f0f571ee96f98f3887069f9c765175` fixes stale realtime protocol
  test fixtures.

The remaining clean-history commit is packet-only rework and does not change
the typed realtime protocol implementation.

## Complexity Delta

- Docs/scripts/tests: PR branch changes realtime tests and this packet.
- Non-test implementation: realtime contracts, RPC host/server, Rust SDK
  realtime helper, TypeScript/Python SDK exports, and SDK codegen.
- Generated/schema churn: 3 files in the current-main diff:
  `artifacts/schemas/rest-openapi.json`,
  `artifacts/schemas/wire-types.json`, and
  `sdks/typescript/src/generated/types.ts`.

## Implementation

- `RealtimeProtocolVersion` is the typed owner for realtime protocol-version
  truth.
- `RealtimeOpenInfo.supported_protocol_versions` is
  `Vec<RealtimeProtocolVersion>`.
- `RealtimeOpenInfo.default_protocol_version` is `RealtimeProtocolVersion`.
- `RealtimeChannelOpenFrame.protocol_version` is `RealtimeProtocolVersion`.
- `RealtimeChannelOpenedFrame.protocol_version` is `RealtimeProtocolVersion`.
- `RealtimeWsHost` keeps supported/default protocol versions typed, issues typed
  open-info, and accepts only typed `channel.open` protocol versions.
- Unknown raw protocol-version strings fail before semantic handling when
  deserializing `RealtimeClientFrame`.
- The Rust SDK preflight compares typed `RealtimeProtocolVersion` values from
  `RealtimeOpenInfo` and `RealtimeChannelOpenFrame`; it no longer parses or
  filters open-info version strings.
- The TypeScript realtime helper sends
  `openInfo.default_protocol_version` through the generated
  `RealtimeProtocolVersion` field.

## Generated, Schema, And SDK Churn

- Regenerated `artifacts/schemas/wire-types.json`.
- Regenerated `artifacts/schemas/rest-openapi.json`.
- Regenerated Python SDK generated types.
- Regenerated TypeScript SDK generated types.
- Added generated SDK aliases for `RealtimeProtocolVersion`.
- Exported `RealtimeProtocolVersion` through the TypeScript and Python public
  SDK type surfaces.
- Updated TypeScript tests to expect generated protocol literal `"2"` instead
  of raw `string`.
- Updated Python realtime SDK fixtures to use the generated
  `RealtimeProtocolVersion` alias and current literal `"2"`.

## Old Path Amputation Proof

| Old path | Classification | Search literal | Mechanical proof | Follow-up |
| --- | --- | --- | --- | --- |
| `RealtimeOpenInfo.supported_protocol_versions` raw string vector | made uncallable at boundary | `supported_protocol_versions: Vec<String>` | Production boundary rejects requests carrying unknown strings before semantic handling. | none |
| `RealtimeOpenInfo.default_protocol_version` raw string field | made uncallable at boundary | `default_protocol_version: String` | Production boundary rejects requests carrying unknown strings before semantic handling. | none |
| `RealtimeChannelOpenFrame.protocol_version` raw string field | made uncallable at boundary | `RealtimeChannelOpenFrame.protocol_version: String` | Production boundary rejects requests carrying unknown strings before host acceptance. | none |
| `RealtimeChannelOpenedFrame.protocol_version` raw string field | made uncallable at boundary | `RealtimeChannelOpenedFrame.protocol_version: String` | Production boundary emits `RealtimeProtocolVersion`; arbitrary strings are not callable output. | none |
| `RealtimeWsHost` raw protocol-version default selection | made uncallable at boundary | `RealtimeWsHost raw protocol-version default selection` | Production boundary rejects requests carrying raw strings before typed default selection. | none |
| TypeScript realtime helper raw protocol-version default selection | made uncallable at boundary | `TypeScript realtime helper raw protocol-version default selection` | SDK boundary compile-fails arbitrary strings before helper selection. | none |

## Retained String Mirrors

- JSON wire encoding of `RealtimeProtocolVersion` remains the stable string
  discriminant `"2"`. This is serialization projection only.
- `RealtimeProtocolVersion::as_str()` and `Display` exist for diagnostics and
  projection into messages. They do not choose defaults or compatibility.
- `RealtimeOpenError::UnsupportedProtocolVersion.requested: String` is an
  internal diagnostic string for a typed-but-unsupported version. It is not used
  for acceptance.
- `RealtimeErrorDetails::UnsupportedProtocolVersion.requested: String` carries
  rejected client input for operators/SDK errors. It is not accepted back as
  protocol truth.
- WebSocket parse error messages may include the rejected raw JSON field value
  through serde diagnostics. These messages are inert.

## Validation

- Pre-edit baseline:
  `./scripts/repo-cargo test -p meerkat-rpc accept_open_frame_returns_typed_protocol_version --lib`
  passed.
- Contract focused:
  `./scripts/repo-cargo test -p meerkat-contracts protocol_version --lib`
  passed.
- RPC typed boundary:
  `./scripts/repo-cargo test -p meerkat-rpc protocol_version --lib` passed.
- RPC typed unsupported host configuration:
  `./scripts/repo-cargo test -p meerkat-rpc accept_open_frame_rejects_expired_role_mismatch_and_typed_unsupported_protocol --lib`
  passed.
- RPC realtime websocket integration:
  `./scripts/repo-cargo test -p meerkat-rpc channel_open_attaches_runtime_and_reports_opening_status --test realtime_ws_protocol`
  passed.
- Rust SDK realtime builder:
  `./scripts/repo-cargo test -p meerkat realtime_channel_connects_and_exchanges_frames --test realtime_channel_builder`
  passed.
- TypeScript SDK:
  `npm_config_package_lock=false make test-sdk-typescript` passed.
- Format/schema:
  `make fmt`, `make fmt-check`, and `make verify-schema-freshness` passed.
- Broad BuildBuddy check:
  `make check` passed via BuildBuddy invocation
  `3cd88754-82fe-4443-b0e4-046fd7802d99`.
- Rework lint:
  `make lint` passed via BuildBuddy invocation
  `32c632ba-2925-472a-95d4-64eaa694d763`.
- Unit-test CI rework:
  `make buildbuddy-test-unit` passed via BuildBuddy invocation
  `6f9d4663-c0b5-4c4e-9cc7-c015d069f5ba`.
- Packet-only rework:
  `git diff --check` passed.

## Sample Passing Old Path Amputation Proof

| Old path | Classification | Search literal | Mechanical proof | Follow-up |
| --- | --- | --- | --- | --- |
| `old/session-route` | deleted | `old/session-route` | No production references remain. | none |

## Sample Failing Old Path Amputation Proof

| Old path | Classification | Search literal | Mechanical proof | Follow-up |
| --- | --- | --- | --- | --- |
| `old/session-route` | wrapped | `old/session-route` | A preferred path exists beside the old route. | none |
