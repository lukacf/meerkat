# LUC-347 Review Readiness Packet

Issue: LUC-347, P1 Mechanical Slice: typed realtime protocol version wire surface.
Branch: `codex/LUC-347-typed-realtime-protocol`.
Base: `origin/main` at `042879d752911a73fb39c8072b01f48b0d292334`.
Review head: the PR head commit containing this packet.

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

- Public open-info/open/opened Rust contracts no longer expose protocol-version
  semantic fields as `String` or `Vec<String>`.
- `RealtimeWsHost` no longer converts supported/default protocol versions to
  strings to issue defaults or make compatibility decisions.
- `accept_open_frame_with_realm` branches on typed
  `RealtimeProtocolVersion`, not parsed strings.
- SDK helper compatibility checks no longer classify supported/default versions
  by parsing open-info strings.
- TypeScript generated contracts reject arbitrary protocol strings at compile
  time through `RealtimeProtocolVersion = "2"`.
- REST realtime proxy fixtures now serialize the typed
  `RealtimeProtocolVersion::CURRENT` value instead of minting protocol strings.
- Rust serde rejects unknown protocol-version strings at the typed frame
  boundary.

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
- RPC focused:
  `./scripts/repo-cargo test -p meerkat-rpc accept_open_frame_returns_typed_protocol_version --lib`
  passed after implementation.
- RPC typed boundary:
  `./scripts/repo-cargo test -p meerkat-rpc protocol_version --lib` passed.
- RPC typed unsupported host configuration:
  `./scripts/repo-cargo test -p meerkat-rpc accept_open_frame_rejects_expired_role_mismatch_and_typed_unsupported_protocol --lib`
  passed.
- Rust SDK realtime builder:
  `./scripts/repo-cargo test -p meerkat realtime_channel_connects_and_exchanges_frames --test realtime_channel_builder`
  passed.
- RPC realtime websocket integration:
  `./scripts/repo-cargo test -p meerkat-rpc channel_open_attaches_runtime_and_reports_opening_status --test realtime_ws_protocol`
  passed.
- TypeScript SDK:
  `npm_config_package_lock=false make test-sdk-typescript` passed.
- Format:
  `make fmt` and `make fmt-check` passed.
- Broad BuildBuddy check:
  `make check` passed via BuildBuddy invocation
  `3cd88754-82fe-4443-b0e4-046fd7802d99`.
- Rework REST proxy fixture:
  `./scripts/repo-cargo test -p meerkat-rest test_realtime_open_info_route_proxies_to_realtime_rpc_host --lib`
  passed.
- Rework contract regression fixtures:
  `./scripts/repo-cargo test -p meerkat-contracts realtime_ --test regression_wire_types`
  passed.
- Rework Python SDK fixtures:
  `PYTHONPATH=sdks/python python3 -m pytest -q sdks/python/tests/test_realtime_channel.py sdks/python/tests/test_types.py`
  passed. `make test-sdk-python` was blocked locally by PEP 668 system-Python
  package installation rules before test execution.
- Rework format:
  `make fmt` and `make fmt-check` passed.
- Rework BuildBuddy unit lane:
  `make buildbuddy-test-unit` passed via BuildBuddy invocation
  `6f9d4663-c0b5-4c4e-9cc7-c015d069f5ba`.
