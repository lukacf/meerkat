# Autonomous Execution Pack

This file turns the sequential phase plan into agent-ready packets. A subsequent autonomous implementation run should be able to pick up this file and execute the phases in order without opening new architecture questions.

## Shared execution rules

- Use spec IDs from [03-requirements-and-rtm.md](./03-requirements-and-rtm.md) in every handoff and review note.
- Update [02-external-contract-matrix.md](./02-external-contract-matrix.md) immediately when a Phase 0 probe changes a verdict.
- Do not replace architecture drift with “temporary” embedded-only logic.
- Keep internal contract probes under `scripts/live_smoke/*`; keep public examples user-facing and pattern-oriented.
- Treat host-sim results as TDD support and hardware results as closing evidence.
- Do not let sacrificial-stack probe evidence close Rust-stack-specific rows; those must be rerun on the planned production stack.
- Store evidence under `artifacts/embedded-esp32/phase-<n>/...`.
- Only metric-positioning rejection has a predeclared fallback in required scope. Peer-discovery or peer-messaging rejection is a hard stop for Example 037.

## Applied field heuristics

These heuristics come from adjacent real ESP32-S3 delivery experience captured in [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md).

- Use tool chains end-to-end: device observation, browser automation, host API checks, network inspection, and flash tooling should be composed rather than treated as separate tasks.
- Verify board and vendor APIs against actual library source or installed headers before writing abstractions around them.
- Exercise backend APIs from the host before flashing firmware that depends on them.
- If hardware is unavailable but the software design is still moving, build a host preview, simulator, or UI mock rather than idling.
- Escalate physical blockers early and stop busy polling when there is no actionable software work left.
- After the first successful Wi-Fi-enabled flash, establish OTA or another low-friction redeploy path as soon as the chosen stack allows it.

## Standard handoff format

Every phase handoff must include:

- `phase`: the completed phase number
- `status`: `pass` or `fail`
- `specs_closed`: list of spec IDs closed in the phase
- `artifacts`: list of artifact paths
- `tests_run`: commands executed and their exit status
- `negative_controls`: commands executed and their verdict
- `open_blockers`: blockers that keep the phase open
- `rtm_updates`: status changes made to RTM rows
- `matrix_updates`: status changes made to matrix rows

## Reviewer model

Each phase should be reviewed independently by three reviewers or reviewer-agents:

- Architecture reviewer: checks invariants, ownership boundaries, and absence of shadow implementations.
- Platform reviewer: checks target-specific evidence, reproducibility, and operational realism.
- Verification reviewer: checks TDD order, negative controls, and proof artifacts.

The review set is mandatory even if the executor is autonomous.

## Packet - Phase 0

### Goal

Close the Phase-0-owned external and repo-model assumptions on real hardware before architecture-prep work begins.

### Required inputs

- Real ESP32-S3 hardware with PSRAM. Use an 8MB-class PSRAM board as the planning baseline unless evidence later relaxes it.
- USB serial access
- Wi-Fi credentials
- Provider API key
- Board flash and monitor toolchain
- This dossier

### Exact tasks

1. Create an internal probe harness under `scripts/live_smoke/esp32-contract-probe/`.
2. Implement a `host-core-check` mode that proves the dossier's Meerkat-side reading by emitting explicit pass or fail markers for three checks: canonical build path (`FactoryAgentBuilder -> SessionService -> AgentFactory::build_agent()`), provider-model expectations (shared request shaping plus incremental stream parsing on the current host path), and runtime-authority expectations (keep-alive, external-event admission, and cancellation owned by `RuntimeSessionAdapter` rather than direct surface shortcuts). This mode may wrap targeted tests or a purpose-built host harness, but it does not count as satisfied by running an unrelated broad workspace test suite without these explicit checks.
3. Implement a `single-node` mode that can boot, connect to Wi-Fi, sync time, hit a provider endpoint, and stream output.
4. Implement a `single-node-rust-stack` mode that boots the planned Rust stack on real hardware and records one-turn resource and scheduler evidence.
5. Add a Meerkat-footprint baseline mode that measures the current pre-fix smallest-runnable agent shapes, including the current facade-linked baseline and the most direct core-agent baseline available without Phase 1 fixes.
6. Discover and validate the required backend APIs from the host before the first target flash that depends on them.
7. Verify any board-specific APIs against actual installed headers or library source before committing the probe implementation.
8. Add stable serial markers for `host-core-check` (`MKT:HOST_CORE:FACTORY_OK`, `MKT:HOST_CORE:PROVIDER_OK`, `MKT:HOST_CORE:RUNTIME_OK`, `MKT:HOST_CORE:PASS`), plus boot, Wi-Fi, time, TLS, provider stream start, provider stream completion, Rust-stack pass/fail, Meerkat baseline pass/fail, and overall pass/fail.
9. Add a small host-sim parser that fails when required markers are missing or arrive in the wrong order.
10. Run the probe modes on real hardware and capture metrics and logs.
11. Establish OTA or another low-friction redeploy path after the first successful Wi-Fi-enabled flash if the probe stack supports it.
12. Execute negative controls and verify deterministic failure markers.
13. Update the external contract matrix with verdicts and artifact paths.

### TDD order

1. Write the marker parser and failing tests first.
2. Add host-core-check assertions and explicit host-core markers and get the parser green.
3. Add boot markers and get the parser green.
4. Add Wi-Fi and time markers and get the parser green.
5. Add TLS and provider reachability and get the parser green.
6. Add streamed output markers and get the parser green.
7. Add Rust-stack viability markers and get the parser green.
8. Add Meerkat baseline-shape measurement and comparison markers and get the parser green.
9. Add memory and timing capture and assert thresholds.
10. Add OTA bootstrap or equivalent redeploy simplification if supported.
11. Add negative controls and assert deterministic failure markers.

### Verification commands

These command names are part of the deliverable contract for the phase and must be implemented if absent.

- `./scripts/live_smoke/esp32-contract-probe/run --mode host-core-check`
- `./scripts/live_smoke/esp32-contract-probe/run --mode single-node --host-sim`
- `./scripts/live_smoke/esp32-contract-probe/run --mode single-node --hardware`
- `./scripts/live_smoke/esp32-contract-probe/run --mode single-node-rust-stack --hardware`
- `./scripts/live_smoke/esp32-contract-probe/run --mode meerkat-baseline --hardware`
- `./scripts/live_smoke/esp32-contract-probe/run --mode negative`

### Evidence bundle requirements

- Raw serial transcript
- Parsed marker report
- Host-core-check marker report tied to `INV-002`, `CONTRACT-001`, and `CONTRACT-002`
- Memory, stack, and latency measurements
- Pre-fix Meerkat baseline comparison report for facade-linked versus direct-core agent shapes
- Toolchain bootstrap log
- Rust-stack feasibility report
- Board identity and environment summary
- Updated matrix rows for every exercised assumption

### Gate blockers

- Any Phase-0-owned High-criticality matrix row still open
- Provider HTTPS not working on real hardware
- Streamed reads not working incrementally
- Memory envelope already outside the intended profile
- The planned Rust stack is still unresolved on real hardware
- The probe has evidence that the current executor/runtime hypothesis itself crashes on target and no replacement experiment has been recorded
- A physical blocker is preventing progress and no escalation has been issued

### Reviewer instructions

- Architecture reviewer: confirm the probe checked the Meerkat-side model with `host-core-check` and did not create a false architecture shortcut.
- Platform reviewer: confirm the probe ran on real hardware and that both sacrificial-stack and Rust-stack evidence are retained where required.
- Verification reviewer: confirm negative controls and parser checks are present.

### Handoff

Use the standard handoff format plus a short “baseline viability” verdict.

## Packet - Phase 1

### Goal

Perform seam extraction and coupling cleanup without introducing embedded-specific semantic drift.

### Required inputs

- Passed Phase 0 handoff
- Closed Phase-0-owned High-criticality matrix rows
- Current factory, session, tool, and provider code paths

### Exact tasks

1. Add a transport contract in `meerkat-client`.
2. Port one provider end-to-end through the transport contract while keeping request construction and stream parsing shared.
3. Split persistent-session orchestration from accidental `redb` and target gating in `meerkat-session`.
4. Extract or add shared host-tool callback glue outside the browser-only runtime code.
5. Make hooks, tools, and skills genuinely optional in the facade and transitive feature graph.
6. Add tests that prove the canonical factory/runtime/session ownership still holds.
7. Add grep- or review-based checks for duplicated provider logic and duplicated runtime/session logic.
8. Add compile and cargo-tree proofs that a minimal embedded-facing build no longer hard-links hooks, tools, or skills.

### TDD order

1. Write failing transport-contract tests.
2. Port one provider and get transport-contract tests green.
3. Write failing persistent-session feature-graph tests.
4. Decouple the persistent service and get those tests green.
5. Write failing host-tool callback contract tests.
6. Extract shared glue and get the callback tests green.
7. Write failing minimal-profile feature-graph tests.
8. Decouple hooks, tools, and skills and get those tests green.
9. Run an architecture review against `INV-001`, `INV-002`, and `CONTRACT-007`.

### Verification commands

- `cargo test -p meerkat-client transport_contract -- --nocapture`
- `cargo test -p meerkat-session persistent_service_contract -- --nocapture`
- `cargo test -p meerkat-web-runtime host_tool_contract -- --nocapture`
- `cargo check -p meerkat --no-default-features --features openai`
- `cargo test -p meerkat -- --nocapture`

### Evidence bundle requirements

- Green transport-contract tests
- Green persistent-service tests
- Green host-tool contract tests
- Green minimal-profile compile and feature-graph proofs
- Review note proving no duplicated provider or runtime logic

### Gate blockers

- Any provider parser or request builder duplicated for embedded
- Persistent-session orchestration still implying a single backend story
- Host-tool callback behavior still trapped inside browser-only code
- Hooks, tools, or skills still hard-linked into the claimed minimal embedded-facing build graph

### Reviewer instructions

- Architecture reviewer: inspect diffs for shadow semantics and direct-path shortcuts.
- Platform reviewer: confirm the seams are suitable for ESP integration instead of being desktop-only abstractions.
- Verification reviewer: confirm tests drive the seam extraction instead of post-hoc coverage.

### Handoff

Use the standard handoff format plus a “phase-1 seam map” that names the new or changed seams.

## Packet - Phase 2

### Goal

Add the embedded surface and ESP backend through the canonical runtime-backed path.

### Required inputs

- Passed Phase 1 handoff
- Shared transport seam
- Shared host-tool glue
- Persistent-session decoupling

### Exact tasks

1. Add `meerkat-embedded-runtime` as the platform-neutral embedded surface shell, but only with real shared surface glue extracted in Phase 1.
2. Add `meerkat-esp-runtime` as the ESP binding crate.
3. Wire the embedded surface through `FactoryAgentBuilder`, `PersistentSessionService`, and `RuntimeSessionAdapter`.
4. Implement the ESP transport against the shared transport seam, including the chosen TLS backend and sync/async bridge.
5. Implement storage and runtime persistence through the existing store traits.
6. Bind host-tool callbacks to the shared callback contract.
7. Define the embedded profile and its deterministic unsupported-capability behavior.
8. Add host-sim and unit tests before hardware smoke.

### TDD order

1. Write failing embedded-surface API tests on host-sim.
2. Make the embedded surface route through the canonical factory/runtime path.
3. Write failing ESP transport and stream-bridge tests and get them green.
4. Write failing persistence and recovery tests and get them green in host-sim.
5. Write failing host-tool tests on the embedded surface and get them green.
6. Add unsupported-capability tests and get them green.
7. Run host-sim smoke through the single-node public example.

### Verification commands

- `cargo test -p meerkat-embedded-runtime -- --nocapture`
- `cargo test -p meerkat-esp-runtime -- --nocapture`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode host-sim`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode build-only`

### Evidence bundle requirements

- Embedded runtime test logs
- ESP backend test logs
- Host-sim smoke artifacts
- Unsupported-capability proof logs

### Gate blockers

- Embedded surface bypasses the runtime-backed path
- ESP backend talks directly to providers outside the shared transport seam
- Unsupported capabilities disappear silently instead of failing deterministically

### Reviewer instructions

- Architecture reviewer: verify `INV-001`, `INV-002`, and `CONTRACT-002`.
- Platform reviewer: verify the ESP backend lives behind transport and store seams.
- Verification reviewer: verify host-sim coverage arrives before hardware smoke.

### Handoff

Use the standard handoff format plus a “surface ownership map” for the new crates.

## Packet - Phase 3

### Goal

Ship the real user-facing examples and use them as the final smoke harnesses.

### Required inputs

- Passed Phase 2 handoff
- Working embedded surface and ESP backend
- Stable marker protocol

### Exact tasks

1. Finalize `examples/036-esp32-event-agent-sh`.
2. Close the swarm viability gate on the real Phase 2 stack and then finalize `examples/037-esp32-triangulation-swarm-sh`.
3. Ensure both examples rebuild repo-local runtime artifacts before building or flashing.
4. Ensure both examples are self-contained real-surface entrypoints.
5. Ensure both examples expose a user-facing `run` mode in addition to test modes.
6. Add host-sim and hardware smoke modes.
7. Add negative-control modes.
8. Add OTA or the best available low-friction redeploy path if the example stack supports it.
9. Add an operator-facing topology report or visualizer for Example 037 using the same inventory and telemetry inputs as the run and smoke modes.
10. If metric positioning is rejected on real hardware, switch Example 037 to the predeclared topology-only fallback contract and prove that path instead of reopening product scope.
11. If peer discovery or bounded-latency messaging is rejected on real hardware, emit an explicit required-scope stop record for Example 037 instead of inventing a replacement example or fallback.
12. Write READMEs that explain the user pattern, setup, redeploy workflow, markers, and proof obligations.
13. Collect artifact bundles from both examples on real hardware.

### TDD order

1. Write failing smoke-parser tests for both examples.
2. Make build-only modes pass.
3. Make user-facing `run` modes pass.
4. Make host-sim modes pass.
5. Make Example 037 produce a stable metric-or-topology artifact in host-sim.
6. Close the swarm viability gate on real hardware.
7. Make hardware smoke modes pass.
8. Add negative controls and make them fail deterministically.
9. Run the full hardware sweep on the phase branch.

### Verification commands

- `./examples/036-esp32-event-agent-sh/examples.sh --mode build-only`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode run`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode host-sim`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode hardware-smoke`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode build-only`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode run --inventory ./nodes.toml`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode host-sim --nodes 4`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode hardware-smoke --inventory ./nodes.toml`

### Evidence bundle requirements

- Raw logs for all smoke runs
- Parsed marker summaries
- README snapshots
- Proof mapping from example markers to `E2E-*`
- Example 036 redeploy notes or OTA evidence when supported
- Example 037 topology report or visualizer output
- Example 037 metric-versus-topology closure verdict

### Gate blockers

- Either example is only a toy harness instead of a real recommended pattern and surface entrypoint
- Either example lacks host-sim or hardware smoke mode
- Either example lacks deterministic negative controls
- Example 037 lacks an operator-facing topology artifact tied to real run data
- Example 037 has neither metric-position proof nor the predeclared topology-only fallback proof
- `ASSUMP-010` is rejected and no explicit required-scope stop record was produced

### Reviewer instructions

- Architecture reviewer: confirm the examples use the canonical surface path.
- Platform reviewer: confirm real hardware smoke exists and is reproducible.
- Verification reviewer: confirm markers, parser, and negative controls are all enforced.

### Handoff

Use the standard handoff format plus an “example proof matrix” mapping markers to `E2E-*`.

## Packet - Final closure

### Goal

Close the remaining proof obligations and freeze the branch as implementation-complete for the required scope.

### Required inputs

- Passed Phase 3 handoff
- Complete artifact bundles
- Current RTM and matrix

### Exact tasks

1. Update every `UNEXECUTED` RTM row with a real verdict.
2. Update every matrix row with its final status and artifact links.
3. Run the freeze checklist.
4. Run the full smoke sweep again on the frozen branch if any core artifact changed during closure.
5. Produce a final artifact index and a branch summary.

### TDD order

1. Treat missing proof artifacts as failing conditions.
2. Add or rerun the missing proof before updating the RTM.
3. Re-run the freeze checklist until it passes without waivers.

### Verification commands

- `rg -n "UNEXECUTED" docs/architecture/embedded-esp32`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode hardware-smoke`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode hardware-smoke --inventory ./nodes.toml`

### Evidence bundle requirements

- Final RTM with no `UNEXECUTED` rows
- Final matrix with no open High-criticality rows and explicit closure notes for any fallback-backed rejection
- Freeze checklist result
- Final artifact index

### Gate blockers

- Any required artifact missing
- Any High-criticality hypothesis still open, or any rejection lacking its predeclared fallback or stop record
- Any remaining shadow-implementation concern

### Reviewer instructions

- Architecture reviewer: confirm invariants and contracts are fully closed.
- Platform reviewer: confirm the final evidence set is real-hardware grounded.
- Verification reviewer: confirm the freeze checklist and RTM are complete.

### Handoff

Use the standard handoff format plus a “program complete” verdict.
