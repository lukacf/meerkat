# Requirements, Contracts, And RTM

This file converts the dossier into a traced specification. IDs here are the authoritative handles for planning, implementation, review, and evidence collection.

## ID policy

- `INV-*` = invariant that must hold across all phases.
- `CONTRACT-*` = architectural or behavioral contract the implementation must preserve.
- `REQ-*` = phase-owned delivery requirement.
- `E2E-*` = externally visible end-to-end proof obligation.
- `CHOKE-*` = mandatory gate or proof lane that can block progress.

## Invariants

| Spec ID | Statement | Owning phase | Runtime caller | Proof artifact | Implementation status | Validation status |
| --- | --- | --- | --- | --- | --- | --- |
| INV-001 | No shadow implementation is allowed: no second agent loop, no second session authority, no second provider parser stack, and no second tool router. | All phases | All surfaces | Architecture review against [01-code-grounded-baseline.md](./01-code-grounded-baseline.md) plus grep-based duplicate-path audit in final closure | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| INV-002 | The canonical embedded semantic path is `Surface -> RuntimeSessionAdapter -> SessionService -> AgentFactory::build_agent()`. | All phases | Embedded surface | Build-path tests and architecture review against canonical path | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| INV-003 | Capability limits are expressed as profile limits and backend limits, not semantic forks. | All phases | Embedded surface and examples | Unsupported-capability tests and embedded profile spec | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| INV-004 | Every current-state claim in the planning dossier is code-grounded and every external assumption has a live-validation path. | Planning output and all phases | Planning dossier | Dossier review plus Phase 0 matrix updates | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| INV-005 | Critical embedded assumptions must be closed with real hardware in the phase that owns them. Phase 0 is hardware-first for baseline board and toolchain assumptions, and Phase 3 is hardware-first for swarm-specific assumptions. Simulator evidence is supplemental and cannot close critical assumptions alone. | Phase 0 and Phase 3 | Phase 0 and example smoke harnesses | Hardware artifact bundle and matrix status updates | `SPECIFIED` | `UNEXECUTED` |
| INV-006 | Sacrificial probe evidence only closes board and external contract questions. Rust-stack-specific behavior must be revalidated on the planned production stack before later phases close. | Phase 0 through final closure | Probe harnesses and final examples | Matrix updates keyed to probe stack and production stack evidence | `SPECIFIED` | `UNEXECUTED` |

## Contracts

| Spec ID | Statement | Owning phase | Runtime caller | Proof artifact | Implementation status | Validation status |
| --- | --- | --- | --- | --- | --- | --- |
| CONTRACT-001 | Agent creation on embedded routes through `FactoryAgentBuilder` and `AgentFactory::build_agent()`. | Phase 1 and Phase 2 | Embedded surface bootstrap | Factory-path tests and code review | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| CONTRACT-002 | Runtime-owned semantics such as keep-alive, event admission, and cancellation remain under `RuntimeSessionAdapter` rather than direct substrate calls. | Phase 2 and Phase 3 | Embedded surface runtime | Runtime-backed turn tests and event-agent smoke | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| CONTRACT-003 | Provider request building, incremental response parsing, retry semantics, and normalized event mapping stay shared. The transport, TLS backend, HTTP client, and sync/async bridge may differ per platform as long as they satisfy the same provider contract. | Phase 1 and Phase 2 | Provider transport | Shared transport contract tests, stream-bridge tests, and one-provider embedded proof | `SPECIFIED` | `UNEXECUTED` |
| CONTRACT-004 | Host-tool callbacks preserve both awaited callback behavior and fire-and-forget acknowledgement behavior. | Phase 1 through Phase 3 | Host tool dispatcher | Shared host-tool tests and smoke examples | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| CONTRACT-005 | Persistence and recovery use `SessionStore`, `RuntimeStore`, and `BlobStore` seams rather than embedded-only session logic. | Phase 1 through Phase 3 | Session persistence and runtime durability | Persistent service tests and reboot smoke | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |
| CONTRACT-006 | Examples `036` and `037` are authoritative user-facing embedded patterns with real operator workflows and supporting artifacts, and they also serve as smoke entrypoints. They are not throwaway demos or internal probe harnesses. | Phase 3 and final closure | Example entrypoints | Example specs, scripts, user-facing artifacts, and hardware smoke artifacts | `SPECIFIED` | `UNEXECUTED` |
| CONTRACT-007 | Hooks, tools, and skills must remain optional subsystems in the build graph for minimal and embedded profiles. The facade and its transitive graph must not hard-link them into a "minimal" build. | Phase 1 and Phase 2 | Facade and embedded profile composition | Feature-graph tests, minimal-build compile proof, and architecture review | `SPECIFIED` | `BASELINE_CODE_GROUNDED` |

## Delivery requirements

| Spec ID | Statement | Owning phase | Runtime caller | Proof artifact | Implementation status | Validation status |
| --- | --- | --- | --- | --- | --- | --- |
| REQ-001 | Phase 0 must prove boot, Wi-Fi, and time-sync readiness on a real ESP32-S3 board. | Phase 0 | Internal probe harness | Hardware contract logs and matrix updates for `ASSUMP-001` | `SPECIFIED` | `LIVE_VALIDATED` |
| REQ-002 | Phase 0 must prove direct provider HTTPS and streamed incremental reads on real hardware. | Phase 0 | Internal probe harness | Hardware stream logs and matrix updates for `ASSUMP-002` and `ASSUMP-003` | `SPECIFIED` | `LIVE_VALIDATED` |
| REQ-003 | Phase 0 must measure memory, stack, and latency envelopes under a real one-turn workload and set the embedded profile thresholds from evidence. | Phase 0 | Internal probe harness and Example 036 | Metrics artifact bundle and matrix update for `ASSUMP-004` | `SPECIFIED` | `LIVE_VALIDATED` |
| REQ-004 | Phase 1 must introduce a platform-agnostic transport seam in `meerkat-client` and port at least one provider through it without duplicating provider logic. | Phase 1 | Provider transport | Transport contract tests and one-provider shared parser proof | `SPECIFIED` | `UNEXECUTED` |
| REQ-005 | Phase 1 must decouple persistent session orchestration from accidental `redb` and target gating so embedded backends can use the real `PersistentSessionService`. | Phase 1 | Session persistence | Feature-graph tests and persistent-service backend tests | `SPECIFIED` | `UNEXECUTED` |
| REQ-006 | Phase 1 must extract or add shared surface glue for host-tool callbacks and fire-and-forget acknowledgement. | Phase 1 | Surface host-tool layer | Shared host-tool tests with callback and ack semantics | `SPECIFIED` | `UNEXECUTED` |
| REQ-007 | Phase 2 must add `meerkat-embedded-runtime` as a platform-neutral embedded surface shell only to the extent that Phase 1 yields real shared surface glue worth owning in a separate crate, and that shell must route through the runtime-backed canonical path. | Phase 2 | Embedded surface bootstrap | Embedded runtime tests and architecture review | `SPECIFIED` | `UNEXECUTED` |
| REQ-008 | Phase 2 must add `meerkat-esp-runtime` implementing ESP transport, storage, bootstrap, and host-tool bindings behind existing seams. | Phase 2 | ESP platform runtime | ESP integration tests and internal-probe build | `SPECIFIED` | `UNEXECUTED` |
| REQ-009 | Phase 2 must define and enforce an embedded profile with explicit capability boundaries and deterministic unsupported-capability errors. | Phase 2 | Embedded surface configuration | Profile tests and unsupported-capability smoke | `SPECIFIED` | `UNEXECUTED` |
| REQ-010 | Phase 3 must ship `examples/036-esp32-event-agent-sh` as the recommended single-node persistent event-agent pattern with keep-alive, host tools, restartable state, and a documented low-friction redeploy workflow when the chosen stack supports it. | Phase 3 | Example 036 | Example script, README, host-sim lane, and hardware smoke artifacts | `SPECIFIED` | `UNEXECUTED` |
| REQ-011 | Phase 3 must ship `examples/037-esp32-triangulation-swarm-sh` as the recommended multi-node triangulation and self-organization pattern. Its primary path is metric relative-position estimation, and its predeclared fallback is a confidence-scored topology or adjacency artifact generated from the same run inputs and telemetry used by the example. | Phase 3 | Example 037 | Example script, README, host-sim lane, multi-node hardware smoke artifacts, and topology report or visualizer output | `SPECIFIED` | `UNEXECUTED` |
| REQ-012 | Final closure must leave every MUST row in this file validated and every critical hypothesis in the matrix closed. | Final closure | Entire program | Completed RTM, completed matrix, and final evidence index | `SPECIFIED` | `UNEXECUTED` |
| REQ-013 | Phase 0 must validate the planned Rust toolchain and executor baseline on real hardware strongly enough to decide whether the current Rust `std` plus Tokio production path remains viable before Phase 1 starts. | Phase 0 | Internal probe harness | Toolchain bootstrap log, Rust-stack probe, and matrix update for `ASSUMP-012` | `SPECIFIED` | `LIVE_VALIDATED` |
| REQ-014 | Phase 0 must capture a pre-fix baseline for the smallest currently-runnable Meerkat agent shapes, including the current facade-linked baseline and the most direct core-agent baseline available without architectural fixes, so Phase 1 has before-and-after memory, size, and latency evidence. | Phase 0 | Internal probe harness | Baseline-build report, binary-size or image-size report, runtime memory and latency report, and explicit comparison notes | `SPECIFIED` | `LIVE_VALIDATED` |
| REQ-015 | Phase 1 must make hooks, tools, and skills genuinely optional in the facade and transitive feature graph so a minimal embedded-facing Meerkat build can compile without them unless explicitly enabled. | Phase 1 | Facade and feature graph | Minimal-profile compile tests, cargo-tree or feature-graph proof, and architecture review keyed to `CONTRACT-007` | `SPECIFIED` | `UNEXECUTED` |
| REQ-016 | Phase 2 must re-validate the P4+C6 production board before hardware smoke begins: C6 Wi-Fi path, Tokio runtime behavior on RISC-V, memory envelope, and `ring`/`rustls` compilation. | Phase 2 | P4+C6 probe harness | P4 re-validation report and matrix update for `ASSUMP-013` | `SPECIFIED` | `UNEXECUTED` |

## End-to-end proof obligations

| Spec ID | Statement | Owning phase | Runtime caller | Proof artifact | Implementation status | Validation status |
| --- | --- | --- | --- | --- | --- | --- |
| E2E-001 | A real ESP32 board (P4+C6 primary, S3 secondary) can run the single-node event-agent pattern, create a session, and complete one streamed provider turn through the embedded surface entrypoint. | Phase 3 | Example 036 | Hardware smoke log with provider stream markers | `SPECIFIED` | `UNEXECUTED` |
| E2E-002 | A real ESP32 board (P4+C6 primary, S3 secondary) can expose host tools, emit fire-and-forget acknowledgement, and subsequently process completion through the normal single-node surface path. | Phase 3 | Example 036 | Hardware smoke logs with tool request, ack, and completion markers | `SPECIFIED` | `UNEXECUTED` |
| E2E-003 | A runtime-backed session can survive restart or reattachment and complete an additional turn from persisted state. | Phase 3 | Example 036 | Restart smoke log plus persisted-state artifact | `SPECIFIED` | `UNEXECUTED` |
| E2E-004 | Unsupported capabilities in the embedded profile fail deterministically without semantic drift. | Phase 2 and Phase 3 | Embedded profile and examples | Negative-control tests and failure-marker logs | `SPECIFIED` | `UNEXECUTED` |
| E2E-005 | A flashed set of ESP32 boards, using P4+C6 as the primary target and S3 as fallback, can discover peers, exchange telemetry, and close Example 037 through either metric relative-position convergence or the predeclared topology-only fallback artifact on real hardware. | Phase 3 | Example 037 | Multi-node hardware smoke log with discovery, telemetry, convergence, and artifact markers | `SPECIFIED` | `UNEXECUTED` |

## Choke points

| Spec ID | Statement | Owning phase | Runtime caller | Proof artifact | Implementation status | Validation status |
| --- | --- | --- | --- | --- | --- | --- |
| CHOKE-001 | Phase 0 cannot pass while any Phase-0-owned High-criticality hypothesis remains open. | Phase 0 | Internal probe harness and matrix review | Updated [02-external-contract-matrix.md](./02-external-contract-matrix.md) | `SPECIFIED` | `LIVE_VALIDATED` |
| CHOKE-002 | Phase 1 cannot pass while provider transport, persistent-session decoupling, shared host-tool glue, or minimal-profile feature gating still require duplicate logic or keep optional subsystems hard-linked into the embedded-facing build graph. | Phase 1 | Architecture-prep tests | Review packet and green architecture-prep tests | `SPECIFIED` | `UNEXECUTED` |
| CHOKE-003 | Phase 2 cannot pass while the embedded surface bypasses the runtime-backed path or lacks explicit profile behavior. | Phase 2 | Embedded runtime tests | Embedded runtime test suite and architecture review | `SPECIFIED` | `UNEXECUTED` |
| CHOKE-004 | Phase 3 cannot pass while examples 036 and 037 are not both runnable as host-sim and hardware smoke entrypoints. | Phase 3 | Example scripts | Example artifact bundle and smoke summary | `SPECIFIED` | `UNEXECUTED` |
| CHOKE-005 | Phase 3 cannot pass while the swarm viability rows remain open or Example 037 lacks either metric-position proof or the predeclared topology-only fallback proof. | Phase 3 | Example 037 and matrix review | Example 037 artifact bundle plus updated [02-external-contract-matrix.md](./02-external-contract-matrix.md) | `SPECIFIED` | `UNEXECUTED` |

## MUST-set summary

The entire program treats every `INV-*`, `CONTRACT-*`, `REQ-*`, `E2E-*`, and `CHOKE-*` row above as `MUST`.

## RTM usage rules

- Phase handoffs must reference spec IDs, not prose summaries.
- Evidence bundles must be keyed by spec ID.
- Matrix updates in [02-external-contract-matrix.md](./02-external-contract-matrix.md) must link back to the owning `REQ-*`, `E2E-*`, or `CHOKE-*` row here.
- Final closure is complete only when every row with `Validation status = UNEXECUTED` has been updated to a real verdict.
