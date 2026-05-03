# LUC-348 Review Readiness Packet

Issue: `LUC-348 P1 Mechanical Slice: typed public mob lifecycle actions`

## Exact-Head Scope

This packet applies to branch `codex/LUC-348-typed-mob-lifecycle` and the
commit that includes this file.

## Implementation Churn

- `meerkat-mob-mcp/src/lib.rs`
  - Added `MobMcpState::mob_lifecycle_action`, a typed dispatch helper that
    accepts `WireMobLifecycleAction`.
  - Agent MCP `mob_lifecycle` now deserializes `MobLifecycleParams` from
    `meerkat-contracts` and dispatches through `mob_lifecycle_action`.
  - The agent MCP `mob_lifecycle` tool schema is emitted from
    `MobLifecycleParams`.
- `meerkat-mob-mcp/src/public_mcp.rs`
  - Public MCP `meerkat_mob_lifecycle` now deserializes `MobLifecycleParams`
    from `meerkat-contracts`.
  - Public MCP lifecycle results are emitted as `MobLifecycleResult`.
  - Public MCP dispatches through `MobMcpState::mob_lifecycle_action`.
- `meerkat-web-runtime/src/lib.rs`
  - WASM `mob_lifecycle(mob_id, action)` keeps its JS string carrier only as
    a compatibility ABI.
  - The string carrier is immediately deserialized into
    `WireMobLifecycleAction` before mob state lookup or dispatch.
  - WASM dispatches through `MobMcpState::mob_lifecycle_action`.

## Generated / Schema Churn

- No generated SDK files changed.
- Existing generated SDK parity remains anchored on the already-generated
  `MobLifecycleParams`, `MobLifecycleResult`, and `MobLifecycleAction` shapes.
- Agent MCP and public MCP lifecycle tool schemas now use the contract schema
  for `MobLifecycleParams` instead of local/manual lifecycle action schemas.

## Test Churn

- `meerkat-contracts/src/wire/mob.rs`
  - Added fixtures proving unknown lifecycle action strings are rejected by
    `MobLifecycleParams` deserialization.
  - Added a typed lifecycle result round-trip fixture.
- `meerkat-mob-mcp/src/lib.rs`
  - Added agent MCP fixtures for unknown action rejection at contract parse
    time and valid typed contract params dispatch.
- `meerkat-mob-mcp/src/public_mcp.rs`
  - Added public MCP fixtures for unknown action rejection at contract parse
    time and valid typed contract params dispatch.
- `meerkat-web-runtime/src/lib.rs`
  - Added a WASM/runtime string-carrier fixture proving the carrier is parsed
    through `WireMobLifecycleAction` and rejects unknown values.

## Old Path Amputation Proof

Removed or made uncallable:

- Public MCP raw lifecycle matcher:
  - Removed `MeerkatMobLifecycleInput { action: String }`.
  - Removed `match input.action.as_str()` over `stop`, `resume`, `complete`,
    `reset`, and `destroy`.
  - Unknown action values now fail during `MobLifecycleParams` deserialization
    before `parse_mob_id`, state lookup, or mob dispatch.
- Agent MCP raw lifecycle matcher:
  - Removed `LifecycleArgs { action: String }`.
  - Removed `match args.action.as_str()` over `stop`, `resume`, `reset`,
    `complete`, and `destroy`.
  - Unknown action values now fail during `MobLifecycleParams` deserialization
    before `MobId` construction or mob dispatch.
- WASM raw lifecycle matcher:
  - Removed local `match action` over `stop`, `resume`, `complete`, `reset`,
    and `destroy`.
  - The exported `action: &str` remains only as the wasm-bindgen JS ABI
    compatibility carrier.
  - Unknown action values now fail during `WireMobLifecycleAction`
    deserialization before `with_mob_state` or mob dispatch.

Remaining raw `action` strings in touched behavior:

- Agent/public MCP and WASM tests still pass JSON/string values to exercise
  wire compatibility. Those carriers deserialize into the typed contract before
  public lifecycle behavior can run.
- Agent MCP still has non-lifecycle raw action strings for `mob_wire` and
  `mob_unwire`. Those are outside the lifecycle action surface and were not
  changed by this slice.

## Validation

- Baseline before edits:
  - `./scripts/repo-cargo test -p meerkat-mob-mcp test_mcp_stop_resume_round_trip --lib`
- Focused validation after edits:
  - `./scripts/repo-cargo test -p meerkat-contracts mob_lifecycle --lib`
  - `./scripts/repo-cargo test -p meerkat-mob-mcp lifecycle --lib`
  - `./scripts/repo-cargo test -p meerkat-web-runtime mob_lifecycle_action_string_carrier --lib`
  - `./scripts/repo-cargo test -p meerkat-mob-mcp test_mcp_stop_resume_round_trip --lib`
- Broad validation:
  - `make check`
  - `make lint`
