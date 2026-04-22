# Lane B Plan

## Owned Rows

- `DOGMA-10`
- `DOGMA-11`
- `DOGMA-21`
- `DOGMA-26`
- `DOGMA-27`

## Owned Milestones

- `B1` = `DOGMA-10`, `DOGMA-11`, `DOGMA-21`, `DOGMA-27`
- `B2` = `DOGMA-26`

## Owned Files

- `meerkat-contracts/src/rest_catalog.rs`
- `meerkat-contracts/src/rpc_catalog.rs`
- `meerkat-mob-mcp/src/public_mcp.rs`
- REST session surface files under `meerkat-rest/src/`
- RPC session/realtime surface files under `meerkat-rpc/src/handlers/`
- SDK wrappers:
  - `sdks/typescript/src/client.ts`
  - `sdks/typescript/src/types.ts`
  - `sdks/python/meerkat/client.py`
  - `sdks/web/src/*`

## Shared Files Touched Under Round Map

- `meerkat-rest/src/lib.rs` as `B1` first owner
- `meerkat-rpc/src/session_runtime.rs` as `B1` first owner
- `meerkat-rpc/src/handlers/session.rs` as `B1` first owner
- `meerkat-rpc/src/realtime_ws.rs` only in `B2` after `EG1a`

## Blocked By

- `B2` blocked by `EG1a`

## Unblocks

- `EG2`

## Public / Type / Schema Changes

- session-control public rename across RPC, REST, and SDKs
- `connection_ref` accepted and forwarded by all create-session surfaces
- public mob MCP spawn/member-send payload cleanup to `member_ref` only
- realtime status semantics parity in `B2`
- schema/catalog/SDK wrapper freshness expected

## Defensive Scan Commitments

- add or update a whole-tree scan banning the old session-control public names

## Requested Orchestrator Probes

- `./scripts/repo-cargo check -p meerkat-contracts`
- `./scripts/repo-cargo check -p meerkat-rpc`
- `./scripts/repo-cargo check -p meerkat-rest`
- `./scripts/repo-cargo check -p meerkat-mob-mcp`
- regen/freshness:
  - `make regen-schemas`
  - `make verify-schema-freshness`
  - `make verify-rpc-surface-alignment`
  - `make verify-sdk-wrapper-freshness`

## Requested Build Gate Commands

- Quick probe `B1`:
  - `./scripts/repo-cargo check -p meerkat-contracts`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
  - `./scripts/repo-cargo check -p meerkat-mob-mcp`
  - `make regen-schemas`
  - `make verify-schema-freshness`
  - `make verify-rpc-surface-alignment`
  - `make verify-sdk-wrapper-freshness`
- Quick probe `B2`:
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-mob-mcp`
  - `make regen-schemas`
  - `make verify-schema-freshness`
  - `make verify-rpc-surface-alignment`
  - `make verify-sdk-wrapper-freshness`
- Full gate `B1`:
  - `./scripts/repo-cargo nextest run -p meerkat-contracts --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-mob-mcp --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rest --lib --show-progress none --status-level none --final-status-level fail`
  - `cd sdks/python && python3 -m pytest tests/test_types.py tests/test_audit_parity.py`
  - `npm --prefix sdks/typescript test`
  - `npm --prefix sdks/web test`
  - `./scripts/repo-cargo e2e-fast`
- Full gate `B2`:
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-mob-mcp --lib --show-progress none --status-level none --final-status-level fail`
  - `cd sdks/python && python3 -m pytest tests/test_types.py`
  - `npm --prefix sdks/typescript test`
  - `./scripts/repo-cargo e2e-fast`

## Risks

- rename work is broad and must be shim-free in handoff commits
- row `27` and SDK/catalog freshness are tightly coupled
- `B2` must not run ahead of the websocket rotation semantics fixed in `EG1a`

## Open Decisions

- none

## Handoff Notes

- handoff commit:
- changed files:
  - `Makefile`
  - `scripts/session_control_public_name_scan.sh`
  - `artifacts/schemas/params.json`
  - `artifacts/schemas/rest-openapi.json`
  - `artifacts/schemas/rpc-methods.json`
  - `artifacts/schemas/wire-types.json`
  - `docs/api/rest.mdx`
  - `docs/api/rpc.mdx`
  - `meerkat-contracts/src/rest_catalog.rs`
  - `meerkat-contracts/src/rpc_catalog.rs`
  - `meerkat-contracts/src/wire/runtime.rs`
  - `meerkat-mob-mcp/src/public_mcp.rs`
  - `meerkat-rest/src/lib.rs`
  - `meerkat-rest/tests/live_rest_matrix.rs`
  - `meerkat-rpc/src/handlers/runtime.rs`
  - `meerkat-rpc/src/handlers/session.rs`
  - `meerkat-rpc/src/router.rs`
  - `meerkat-rpc/src/realtime_ws.rs`
  - `meerkat-rpc/tests/regression_rpc.rs`
  - `meerkat-rpc/tests/integration_server.rs`
  - `meerkat-rpc/tests/live_smoke_rpc.rs`
  - `meerkat-rpc/tests/tcp_e2e.rs`
  - `sdks/typescript/src/client.ts`
  - `sdks/typescript/src/types.ts`
  - `sdks/python/meerkat/client.py`
  - `sdks/python/meerkat/generated/types.py`
  - `sdks/python/tests/test_types.py`
  - `sdks/python/tests/test_audit_parity.py`
  - `sdks/python/tests/test_e2e_smoke.py`
  - `tools/sdk-codegen/generate.py`
- generated output request:
  - `none` for B1 handoff; committed generated/docs/schema fallout was updated in-tree
  - optional confirmation only: `make verify-schema-freshness`
- known risks:
  - `B2` static fix is limited to websocket projection parity: `replacement_pending` / `reattach_required` now surface `attempt_count = 1` like RPC/public MCP
  - Python syntax checks passed; `session-control` gate, RPC surface alignment, and SDK wrapper freshness all passed
  - TypeScript typecheck still could not be run locally because `sdks/typescript/node_modules` is absent and `npm exec tsc` resolved to the wrong package
  - no Cargo/tests were run for B2; orchestrator should still run the planned `meerkat-rpc` / `meerkat-mob-mcp` probes and focused realtime websocket tests
