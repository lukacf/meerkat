# Initial Traceability Matrix

This is the initial requirement traceability matrix for the `0.5` RCT package.

At package creation time, requirements are tracked as `MISSING` unless the
runtime branch already proves them through a shipped real-entrypoint path.
This package intentionally does not overclaim partial progress.

| REQ-ID | Domain | Planned Phase | Runtime Caller / Entry Point | Primary Evidence Target | Initial Status |
| --- | --- | --- | --- | --- | --- |
| `REQ-001` | in-repo source of truth | `2` | repo CI / `cargo xtask machine-check-drift --all` | machine bundle import + CI failure proof | `MISSING` |
| `REQ-002` | schema/codegen workflow | `2` | `cargo xtask machine-codegen --all` | drift and generation proof | `MISSING` |
| `REQ-003` | machine final modes and owner map | `11` | full CI + release gate | final owner-map verification | `MISSING` |
| `REQ-004` | closed runtime ingress taxonomy | `3` | runtime admission path | runtime ingress tests + no-Projected/no-SystemGenerated proof | `MISSING` |
| `REQ-005` | contributor model and receipt semantics | `3` | persistent + ephemeral runtime recovery | replay/receipt tests | `MISSING` |
| `REQ-006` | out-of-band control plane | `3` | runtime control channel | control-plane preemption tests | `MISSING` |
| `REQ-007` | runtime policy algebra and queue discipline | `3` | runtime policy resolution | policy/queue behavioral tests | `MISSING` |
| `REQ-008` | turn execution narrowing | `5` | runtime -> core run primitive path | host-mode cutover + turn kernel proof | `MISSING` |
| `REQ-009` | peer comms normalization + reservations | `5` | `RuntimeCommsBridge` | peer/runtime bridge tests | `MISSING` |
| `REQ-010` | completion-aware runtime comms bridge | `5` | `accept_input_with_completion(...)` | reservation/completion E2E | `MISSING` |
| `REQ-011` | shared async-op lifecycle substrate | `4` | `OpsLifecycleRegistry` | registry-backed mob/background-op proof | `MISSING` |
| `REQ-012` | mob-only child-agent path + no lifecycle leakage | `4` | mob control plane and parent wait path | no `SubagentResult` leakage proof | `MISSING` |
| `REQ-013` | mob owner split | `7` | mob actor/orchestrator runtime path | decomposition and replay tests | `MISSING` |
| `REQ-014` | flow/orchestrator durable truth | `7` | flow run + orchestration kernels | flow/mob replay tests | `MISSING` |
| `REQ-015` | external tool lifecycle machine | `6` | MCP router/adapter/gateway path | add/remove/reload behavioral tests | `MISSING` |
| `REQ-016` | typed notices primary, transcript notices derivative | `6` | runtime/tool-surface notice path | outward notice contract proof | `MISSING` |
| `REQ-017` | CLI/REST/RPC external-event cutovers | `8` | CLI, REST, JSON-RPC | real surface ingress E2Es | `MISSING` |
| `REQ-018` | MCP run/resume cutover | `9` | MCP server tools | runtime-backed MCP smoke | `MISSING` |
| `REQ-019` | WASM/browser convergence | `9` | browser runtime exports | browser E2Es + wasm32 checks | `MISSING` |
| `REQ-020` | Python/TypeScript wrapper parity | `10` | SDK client/wrapper APIs | regenerated bindings + wrapper smoke | `MISSING` |
| `REQ-021` | Rust docs/examples/API posture | `10` | docs/examples compile path | runtime-backed docs/examples proof | `MISSING` |
| `REQ-022` | final deletion ledger complete | `11` | release gate | bypass deletion proof | `MISSING` |
| `REQ-023` | PureHand harness and owner tests | `6` and `7` | `cargo xtask machine-verify --machine <name>` | harness + kernel tests | `MISSING` |
| `REQ-024` | SchemaKernel by-construction workflow | `2` and `3` | generated owner crates | generated kernel no-drift proof | `MISSING` |
| `REQ-025` | surface compatibility/versioning policy | `8` to `10` | public schemas + docs + bindings | change propagation proof | `MISSING` |
