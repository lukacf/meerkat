# Final Traceability Matrix

This is the release-closure traceability matrix for the `0.5` RCT package.

This matrix tracks the implementation/cutover evidence that now backs the
checked-in target-model baseline at commit `4e350279`. Prior phases closed the
runtime-facing seams; phase 11 closes the final machine-inventory,
compatibility-audit, and release-bundle rows.

| REQ-ID | Domain | Planned Phase | Runtime Caller / Entry Point | Primary Evidence Target | Final Status |
| --- | --- | --- | --- | --- | --- |
| `REQ-001` | in-repo source of truth | `2` | repo CI / `cargo xtask machine-check-drift --all` | checked-in `specs/machines/` bundle + drift gate | `VALIDATED` |
| `REQ-002` | machine-authority/codegen workflow | `2` | `cargo xtask machine-codegen --all` | generation, verify, and drift repo entrypoints | `VALIDATED` |
| `REQ-003` | machine final modes and owner map | `11` | `cargo xtask machine-verify --all` | final machine inventory in `specs/machines/README.md` + release gate | `VALIDATED` |
| `REQ-004` | closed runtime ingress taxonomy | `3` | runtime admission path | runtime ingress tests + no-Projected/no-SystemGenerated proof | `VALIDATED` |
| `REQ-005` | contributor model and receipt semantics | `3` | persistent + ephemeral runtime recovery | replay/receipt tests across runtime/store combinations | `VALIDATED` |
| `REQ-006` | out-of-band control plane | `3` | runtime control channel | control-plane preemption and completion-resolution tests | `VALIDATED` |
| `REQ-007` | runtime policy algebra and queue discipline | `3` | runtime policy resolution | policy/queue behavioral tests + machine alignment | `VALIDATED` |
| `REQ-008` | turn execution narrowing | `5` | runtime -> core run primitive path | host-mode cutover + turn-execution machine proof | `VALIDATED` |
| `REQ-009` | peer comms normalization + reservations | `5` | `RuntimeCommsBridge` | peer/runtime bridge tests | `VALIDATED` |
| `REQ-010` | completion-aware runtime comms bridge | `5` | `accept_input_with_completion(...)` | reservation/completion bridge proof | `VALIDATED` |
| `REQ-011` | shared async-op lifecycle substrate | `4` | `OpsLifecycleRegistry` | registry-backed mob/background-op proof | `VALIDATED` |
| `REQ-012` | mob-only child-agent path + no lifecycle leakage | `4` | mob control plane and parent wait path | no `SubagentResult` leakage proof | `VALIDATED` |
| `REQ-013` | mob owner split | `7` | mob actor/orchestrator runtime path | decomposition and replay tests | `VALIDATED` |
| `REQ-014` | flow/orchestrator durable truth | `7` | flow run + orchestration kernels | flow/mob replay tests | `VALIDATED` |
| `REQ-015` | external tool lifecycle machine | `6` | MCP router/adapter/gateway path | add/remove/reload behavioral tests | `VALIDATED` |
| `REQ-016` | typed notices primary, transcript notices derivative | `6` | runtime/tool-surface notice path | outward typed notice/delta contract proof | `VALIDATED` |
| `REQ-017` | CLI/REST/RPC external-event cutovers | `8` | CLI, REST, JSON-RPC | real surface ingress E2Es | `VALIDATED` |
| `REQ-018` | MCP run/resume cutover | `9` | MCP server tools | runtime-backed MCP smoke | `VALIDATED` |
| `REQ-019` | WASM/browser convergence | `9` | browser runtime exports | browser E2Es + wasm32 checks | `VALIDATED` |
| `REQ-020` | Python/TypeScript wrapper parity | `10` | SDK client/wrapper APIs | regenerated bindings + wrapper smoke | `VALIDATED` |
| `REQ-021` | Rust docs/examples/API posture | `10` | docs/examples compile path | runtime-backed docs/examples proof | `VALIDATED` |
| `REQ-022` | final deletion ledger complete | `11` | release gate bundle | deletion sweep + compatibility audit + release bundle | `VALIDATED` |
| `REQ-023` | rich-machine authority harness and owner tests | `6` and `7` | `cargo xtask machine-verify --machine <name>` | advanced machine-authority harness + kernel tests | `VALIDATED` |
| `REQ-024` | SchemaKernel by-construction workflow | `2` and `3` | generated owner crates | catalog authority + generated kernel no-drift proof | `VALIDATED` |
| `REQ-025` | surface compatibility/versioning policy | `8` to `11` | public schemas + docs + bindings | change-propagation proof + final compatibility audit | `VALIDATED` |

## Phase 1 Target-Definition Suites Landed

- `xtask/tests/machine_workflow.rs` defines the machine-workflow red-ok drift target.
- `meerkat-runtime/tests/runtime_ingress_control.rs` and `meerkat-runtime/tests/recovery_replay.rs` define runtime ingress/recovery chokepoints.
- `meerkat-comms/tests/runtime_bridge.rs`, `meerkat-core/tests/subagent_wait.rs`, and `meerkat-core/tests/ops_lifecycle_integration.rs` define comms/wait/shared-lifecycle Phase 1 targets.
- `meerkat-mob/tests/ops_registry_integration.rs` and `meerkat-mob/tests/mob_decomposition.rs` define shared-lifecycle and mob decomposition targets.
- `meerkat-mcp/tests/external_tool_integration.rs` and `meerkat-core/tests/tool_notice_projection.rs` define typed external-tool notice targets.
- `meerkat-cli/tests/runtime_backed_ingress.rs`, `meerkat-rest/tests/runtime_backed_ingress.rs`, `meerkat-rpc/tests/runtime_backed_ingress.rs`, and `meerkat-mcp-server/tests/runtime_backed_ingress.rs` define server-surface runtime-backed entrypoints.
- `meerkat-web-runtime/tests/release_targets.rs`, `sdks/python/tests/test_phase1_targets.py`, and `sdks/typescript/tests/release_parity.test.js` define browser/SDK/release target stubs for later implementation phases.

## Final Machine Inventory

Phase 11 closes the machine-mode audit against the checked-in machine bundle:

- every canonical machine listed in [specs/machines/README.md](/Users/luka/src/meerkat/specs/machines/README.md) now targets final mode `SchemaKernel`
- the checked-in Rust catalog under `meerkat-machine-schema/src/catalog/` is the authority for every canonical machine
- the generated kernel under `meerkat-machine-kernels/src/generated/` is the explicit transition boundary consumed by `cargo xtask machine-verify --all`
- no canonical machine is carried forward as `BoundaryRedesign` or `SchemaExtension`

## Release-Gate Evidence Bundle

The final release-closure bundle for `E2E-009` is gate-owned and intentionally
runs as one explicit command set:

1. `cargo nextest run --workspace --all-targets`
2. `cargo xtask machine-verify --all`
3. `cargo xtask machine-check-drift --all`
4. `cargo check -p meerkat-machine-schema --target wasm32-unknown-unknown`
5. `cargo check -p meerkat-machine-kernels --target wasm32-unknown-unknown`

This bundle is the final evidence target for:

- one ordinary runtime path
- one owner per temporal concern
- all canonical machines verified in final mode
- all surface smoke and docs/example parity remaining green
- deletion-ledger closure with no surviving bypass owner
