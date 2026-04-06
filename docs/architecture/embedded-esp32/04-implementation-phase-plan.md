# Sequential Implementation Phase Plan

This file derives the implementation order from the traced requirements, not from crate names. The program is sequential and exhaustive: each phase either closes its scope or forces an immediate plan update before the next phase begins.

## Phase ladder

| Phase | Goal | Primary spec coverage | Entry gate | Exit gate |
| --- | --- | --- | --- | --- |
| Phase 0 | Close single-node target contracts and Meerkat-model assumptions on real hardware before architecture changes commit the project to bad assumptions. | `REQ-001`, `REQ-002`, `REQ-003`, `REQ-013`, `REQ-014`, `CHOKE-001` | Hardware, credentials, and internal probe harness can be executed | All Phase-0-owned High-criticality rows are closed |
| Phase 1 | Perform architecture prep and seam extraction without introducing embedded-specific shadow logic. | `REQ-004`, `REQ-005`, `REQ-006`, `REQ-015`, `CHOKE-002` | Phase 0 passed | Shared transport, persistent-service decoupling, host-tool glue, and real minimal-profile feature gating exist and are test-backed |
| Phase 2 | Assemble the real embedded surface and ESP backend through the canonical runtime-backed path, targeting ESP32-P4+C6 as the primary production board. | `REQ-007`, `REQ-008`, `REQ-009`, `REQ-016`, `CHOKE-003` | Phase 1 passed and P4+C6 boards available | Embedded runtime path, ESP backend, embedded profile, and P4+C6 re-validation are all green |
| Phase 3 | Deliver authoritative user-facing examples that are also full E2E smoke tests, including swarm viability closure on the real embedded stack. | `REQ-010`, `REQ-011`, `E2E-001`, `E2E-002`, `E2E-003`, `E2E-004`, `E2E-005`, `CHOKE-004`, `CHOKE-005` | Phase 2 passed | Examples 036 and 037 pass host-sim and hardware smoke, and the swarm rows are closed |
| Final closure | Remove all provisional state from the required scope and freeze artifacts. | `REQ-012` plus every remaining `UNEXECUTED` row | Phase 3 passed | Full RTM and matrix are closed; freeze checklist passes |

## Phase 0 - External contract validation

### Goal

Close the single-node target assumptions and Meerkat-model assumptions that can invalidate the whole program before architecture refactors begin.

### Covers

- `REQ-001`
- `REQ-002`
- `REQ-003`
- `REQ-013`
- `REQ-014`
- `INV-005`
- `INV-006`
- `CHOKE-001`
- `ASSUMP-001`
- `ASSUMP-002`
- `ASSUMP-003`
- `ASSUMP-004`
- `ASSUMP-007`
- `ASSUMP-008`
- `ASSUMP-012`

### Required outputs

- An internal real-board probe harness under `scripts/live_smoke/esp32-contract-probe/` that emits stable markers.
- A conscious decision on the fastest sacrificial probe stack for Phase 0, separate from the production implementation stack.
- A Rust-stack feasibility verdict for the planned `std` plus Tokio baseline on real hardware.
- A toolchain bootstrap record that shows how the real target environment is reproduced.
- A before-fix baseline report for the current smallest-runnable Meerkat agent shapes, including at least the current facade-linked baseline and the most direct core-agent baseline available without Phase 1 changes.
- Updated matrix verdicts in [02-external-contract-matrix.md](./02-external-contract-matrix.md).
- Metrics and raw logs stored in a predictable artifact path.
- A baseline decision confirmation that native embedded remains viable.

### Behavioral done-when

- The single-node probe boots, joins Wi-Fi, syncs time, reaches a provider endpoint, and completes a streamed probe.
- The probe includes a host-core check that verifies the current repo-side assumptions about the factory, provider, and runtime path before target-specific work is trusted.
- The planned Rust stack is exercised on real hardware strongly enough to decide whether the current production path remains viable.
- The probe records peak memory, stack, and latency envelopes.
- The probe records a pre-fix “too-fat baseline” for the current facade-linked Meerkat path and contrasts it with the most direct core-agent path available before Phase 1 decoupling.
- Backend APIs and service endpoints have been exercised from the host before the first target flash that depends on them.
- No Phase-0-owned High-criticality assumption remains open.

### Real-entrypoint proof

- `./scripts/live_smoke/esp32-contract-probe/run --mode host-core-check`
- `./scripts/live_smoke/esp32-contract-probe/run --mode single-node --hardware`
- `./scripts/live_smoke/esp32-contract-probe/run --mode single-node-rust-stack --hardware`

### Negative control

- Run the same probe harness with deliberately invalid prerequisites such as bad credentials, missing time sync, or a broken Rust-stack bootstrap and verify failure markers are emitted deterministically.

### Operational notes

- Phase 0 is allowed to use a sacrificial vendor-native toolchain if it closes external contracts faster.
- Board or library APIs used in Phase 0 should be verified against actual installed headers or source, not only secondary docs.
- If the board supports OTA or another low-friction redeploy path, establish it as soon as the first Wi-Fi-enabled flash succeeds.
- Sacrificial probe results do not close Rust-stack-specific rows by themselves; the planned Rust stack must be exercised separately before Phase 1 begins.
- If the run is blocked on a physical prerequisite with no remaining software work, escalate the blocker instead of staying in a noisy polling loop.
- Swarm and RSSI viability are intentionally excluded from Phase 0 and are owned by Phase 3 with Example 037.

### Evidence bundle

- Raw serial log
- Parsed marker summary
- Memory and timing report
- Toolchain bootstrap log
- Rust-stack feasibility report
- Board identity and environment notes
- Matrix status updates with artifact paths

## Phase 1 - Architecture prep and seam extraction

### Goal

Fix the accidental coupling that blocks embedded support while keeping the existing ownership model intact.

### Covers

- `REQ-004`
- `REQ-005`
- `REQ-006`
- `REQ-015`
- `INV-001`
- `INV-002`
- `CONTRACT-001`
- `CONTRACT-003`
- `CONTRACT-004`
- `CONTRACT-005`
- `CONTRACT-007`
- `CHOKE-002`

### Required outputs

- Shared provider transport seam in `meerkat-client`
- Real persistent-session orchestration decoupled from accidental target/backend coupling
- Target-aware async trait and feature-graph cleanup where session or store seams currently assume desktop-style `Send` or `redb` ownership
- Owning-crate ESP wake and completion semantics in `meerkat-runtime`, including the real `espidf` wake-delivery behavior proven in Phase 0
- Shared host-tool callback glue, reusable outside the browser runtime
- Owning-crate runtime shaping for ESP-target comms wait or completion behavior, rather than a probe-only drain shim
- Hooks, tools, and skills made genuinely optional in the facade and transitive feature graph for minimal and embedded profiles
- Desktop and unit tests proving no duplicate logic was introduced

### Behavioral done-when

- At least one provider runs through the new transport seam with shared request building and stream parsing.
- `PersistentSessionService` can be used without implying `redb` as the only durable story, and the same service can compile for the ESP target with non-desktop backend choices.
- Host-tool callback behavior is preserved outside browser-only code.
- A minimal embedded-facing Meerkat build can compile without hooks, tools, and skills unless explicitly enabled.
- The owning runtime path preserves the Phase 0 first-turn fix on `espidf` without relying on probe-local wake shims or validator-only sequencing hacks.
- Architecture review shows no shadow loops, no shadow stores, and no duplicated provider logic.

### Real-entrypoint proof

- The existing host and test harnesses can exercise the extracted transport seam and host-tool contract without any ESP-specific runtime yet.

### Negative control

- Introduce a deliberately incompatible transport or disabled persistent backend in tests and verify that the phase fails at the seam rather than silently bypassing it.

### Evidence bundle

- Green transport-contract tests
- Green persistent-service feature-graph tests
- Green host-tool contract tests
- Green minimal-profile compile tests and feature-graph proof
- Architecture review summary keyed to `INV-*` and `CONTRACT-*`

## Phase 2 - Embedded surface and ESP backend

### Goal

Add the real embedded surface and ESP backend through the existing runtime-backed semantics.

### Covers

- `REQ-007`
- `REQ-008`
- `REQ-009`
- `INV-003`
- `CONTRACT-001`
- `CONTRACT-002`
- `CONTRACT-003`
- `CONTRACT-004`
- `CONTRACT-005`
- `E2E-004`
- `CHOKE-003`
- `ASSUMP-005`
- `ASSUMP-006`
- `ASSUMP-009`
- `ASSUMP-013`
- `REQ-016`

### Required outputs

- P4+C6 re-validation report (C6 Wi-Fi path, Tokio re-probe, memory re-measure) before hardware smoke begins
- `meerkat-embedded-runtime` for any real shared surface glue extracted in Phase 1
- `meerkat-esp-runtime`
- Embedded profile definitions and deterministic unsupported-capability behavior
- ESP storage, bootstrap, transport, and host-tool bindings behind existing seams
- Explicit ESP baseline policy for PSRAM, flash geometry and partition sizing, pthread stack sizing, and watchdog-compatible scheduling
- Explicit embedded policy for when the Meerkat lane may stream provider responses versus when the embedded profile must stay on the current non-streaming ESP conversation shape
- Explicit embedded scripting-tool policy using the shared host-tool callback contract, with MicroPython treated as a proven on-device tool pattern rather than a separate runtime

### Behavioral done-when

- The P4+C6 re-validation has passed: C6 Wi-Fi path works, Tokio runtime behavior is characterized on RISC-V, and the memory envelope is measured. `ASSUMP-013` is `LIVE_VALIDATED`.
- The embedded surface bootstraps through `FactoryAgentBuilder`, `PersistentSessionService`, and `RuntimeSessionAdapter`.
- The ESP backend satisfies the surface through transport, persistence, and host-tool bindings instead of direct semantic shortcuts.
- The embedded profile is explicit and test-backed.
- The surface can be exercised in host-sim mode before hardware smoke.
- Host-sim or preflight evidence moves `ASSUMP-005`, `ASSUMP-006`, and `ASSUMP-009` to at least `PROVISIONAL` before Phase 3 attempts real-hardware closure.
- The embedded baseline accounts explicitly for PSRAM-backed heap, pthread stack policy, and watchdog-friendly execution rather than leaving them as probe-only tweaks.
- The embedded baseline also owns the real board flash-size and partition-layout policy instead of inheriting a too-small default image envelope from generic ESP templates.
- The embedded runtime path preserves the owning `meerkat-runtime` comms-drain behavior proven in Phase 0 instead of reintroducing a surface-local or probe-local drain implementation.
- The embedded runtime path also preserves the owning `RuntimeSessionAdapter` wake behavior proven on ESP in Phase 0 instead of reintroducing opportunistic wake delivery that can lose the first peer-triggered turn.
- The embedded profile does not silently assume ESP-side Meerkat streaming is safe just because direct provider streaming was proven separately in Phase 0 or host-side Meerkat streaming later passed in the split rerun.
- The embedded profile explicitly distinguishes memory-backed persistent-session orchestration, which is proven, from restart durability, which is not yet closed.
- Example 036 explicitly tests that peer-chat turns continue to route through the standard `send` tool path under the recommended prompts, so a missed tool call is caught as a failing runtime behavior rather than disappearing into a hung smoke run.
- Example 036 also exercises the host-side persistent-session path, not just the device-side one, because Phase 0 proved that both sides can stay on the standard runtime-backed path in the bounded peer-chat baseline.

### Real-entrypoint proof

- `examples/036-esp32-event-agent-sh/examples.sh --mode host-sim`

### Negative control

- Exercise at least one unsupported capability path and verify a stable failure marker or deterministic error instead of silent omission.

### Evidence bundle

- Embedded runtime test suite
- ESP backend test suite
- Host-sim smoke logs
- Unsupported-capability proofs

## Phase 3 - ESP examples as smoke tests

### Goal

Ship the real examples that also serve as authoritative smoke-entrypoints for CI and hardware validation.

### Covers

- `REQ-010`
- `REQ-011`
- `INV-005`
- `E2E-001`
- `E2E-002`
- `E2E-003`
- `E2E-004`
- `E2E-005`
- `CONTRACT-002`
- `CONTRACT-006`
- `CHOKE-004`
- `CHOKE-005`
- `ASSUMP-005`
- `ASSUMP-006`
- `ASSUMP-009`
- `ASSUMP-010`
- `ASSUMP-011`

### Required outputs

- `examples/036-esp32-event-agent-sh`
- `examples/037-esp32-triangulation-swarm-sh`
- Host-sim and hardware smoke modes
- Stable marker protocol and artifact capture
- A documented low-friction redeploy path for Example 036 when the chosen stack supports it
- An operator-facing topology report or visualizer for Example 037

### Behavioral done-when

- Example 036 proves the recommended single-node persistent event-agent pattern.
- Example 036 also proves repeated embedded scripting-tool calls over the standard host-tool callback contract as part of the recommended single-node pattern.
- Example 037 proves the recommended multi-node triangulation and self-organization pattern through either metric convergence or the predeclared topology-only fallback.
- Example 036 documents or demonstrates the best available low-friction redeploy workflow for the chosen stack.
- Example 037 produces a user-facing topology artifact from the same run data used for validation.
- Both examples follow the repo-local rebuild pattern and the self-contained real-surface pattern.
- Both examples run in build-only, host-sim, and hardware smoke modes.
- Example 036 smoke explicitly checks repeated tool completions, transcript continuity, clean `DISMISS` teardown, and no first-turn stall after reset.
- If `ASSUMP-010` is rejected on the real Phase 3 stack, the program records a required-scope stop for Example 037 instead of inventing a replacement multi-node pattern.

### Real-entrypoint proof

- `./examples/036-esp32-event-agent-sh/examples.sh --mode hardware-smoke`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode hardware-smoke`

### Negative control

- Run each example in a misconfigured mode that should fail deterministically and assert that the script exits non-zero with stable failure markers. For Example 037, verify both that metric-positioning rejection falls back to the predeclared topology contract and that discovery or messaging rejection is treated as a hard required-scope stop rather than a fallback case.

### Evidence bundle

- Example READMEs
- Example scripts
- Host-sim smoke logs
- Hardware smoke logs
- Parsed marker summaries
- Example 036 redeploy notes or evidence when supported by the stack
- Example 037 topology report or visualizer output

## Final closure phase

### Goal

Finish the required scope, close any remaining provisional state, and freeze the dossier and evidence set.

### Covers

- `REQ-012`
- `INV-004`
- every remaining `UNEXECUTED` row in [03-requirements-and-rtm.md](./03-requirements-and-rtm.md)

### Behavioral done-when

- Every `MUST` row in the RTM has a real validation verdict.
- Every High-criticality matrix row is closed, and any `REJECTED` row is paired with its predeclared fallback or an explicit program-stop record.
- No required-scope item is parked as “do this afterward”.
- The freeze checklist passes with real artifact links.

### Real-entrypoint proof

- A full hardware smoke sweep of both public examples on the same frozen branch and artifact index.

### Negative control

- Deliberately run the freeze checklist before one required artifact is present and verify that the dossier is treated as not ready.

### Evidence bundle

- Final RTM
- Final matrix
- Freeze checklist result
- Final artifact index

## Phase transition policy

- No phase may start with an open blocker from the preceding phase.
- A rejected external assumption in Phase 0 stops the program until the plan is revised.
- A failed choke point in any phase means the phase remains open.
- There is no backlog lane for core scope. If a required item is not done, the current phase remains active.
- Only `ASSUMP-011` has a predeclared fallback. A rejected `ASSUMP-010` is a required-scope stop for Example 037.
