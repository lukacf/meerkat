# Embedded ESP32 Planning Dossier

This directory is the canonical internal planning dossier for the ESP32-S3 embedded Meerkat program.

`docs/docs.json` intentionally remains unchanged. The material here is internal planning authority, not public Mintlify navigation, and must stay decision-complete enough for a subsequent autonomous implementation run to execute sequentially from Phase 0 through final closure without making architecture decisions on the fly.

## Dossier rules

1. Every repo-current-state claim is either `CODE_GROUNDED` with direct repo evidence or `HYPOTHESIS` with a live contract test that can prove or reject it. Verified platform and toolchain constraints live separately in [09-technical-dependency-analysis.md](./09-technical-dependency-analysis.md) so they do not masquerade as repo claims.
2. Embedded is a surface and backend set, not a shadow runtime model.
3. The canonical semantic path stays runtime-backed: `Surface -> RuntimeSessionAdapter -> SessionService -> AgentFactory::build_agent()`.
4. Capability limits are profile limits and backend limits, not semantic forks.
5. Phase 0 is hardware-first on a real ESP32-S3 board. Simulator evidence is useful but cannot close critical assumptions by itself.
6. There are no core-scope deferrals. If a phase exposes a blocker, the plan must absorb it immediately instead of pushing it to a backlog.

## Reader order

1. [01-code-grounded-baseline.md](./01-code-grounded-baseline.md)
2. [09-technical-dependency-analysis.md](./09-technical-dependency-analysis.md)
3. [10-phase-0-live-findings.md](./10-phase-0-live-findings.md)
4. [02-external-contract-matrix.md](./02-external-contract-matrix.md)
5. [03-requirements-and-rtm.md](./03-requirements-and-rtm.md)
6. [04-implementation-phase-plan.md](./04-implementation-phase-plan.md)
7. [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md)
8. [06-example-smoke-specs.md](./06-example-smoke-specs.md)
9. [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md)
10. [07-freeze-checklist.md](./07-freeze-checklist.md)

## Authority map

| Question | Authority | Notes |
| --- | --- | --- |
| What does the current codebase already guarantee? | [01-code-grounded-baseline.md](./01-code-grounded-baseline.md) | All rows in this file are `CODE_GROUNDED`. |
| Which non-repo platform or toolchain facts already constrain the plan before live probing starts? | [09-technical-dependency-analysis.md](./09-technical-dependency-analysis.md) | This is the planning-constraint authority for reqwest/rustls, Xtensa tooling, ESP-IDF HTTP behavior, and Tokio-fit concerns. |
| What did the real Phase 0 run on the baseline board actually prove? | [10-phase-0-live-findings.md](./10-phase-0-live-findings.md) | This is the authoritative results log for the completed Phase 0 spike. |
| Which external assumptions can sink the project and how do we test them? | [02-external-contract-matrix.md](./02-external-contract-matrix.md) | All assumption rows are `HYPOTHESIS` until Phase 0 evidence updates them. |
| What are the immutable invariants, contracts, requirements, E2E obligations, and choke points? | [03-requirements-and-rtm.md](./03-requirements-and-rtm.md) | This is the requirement-trace authority. |
| What is the sequential implementation order and the gate for each phase? | [04-implementation-phase-plan.md](./04-implementation-phase-plan.md) | This file is requirement-driven, not component-driven. |
| What should an autonomous coding agent do phase by phase? | [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md) | This is the execution choreography. |
| What exactly must `examples/036` and `examples/037` teach and prove? | [06-example-smoke-specs.md](./06-example-smoke-specs.md) | These examples are user-facing patterns first and smoke entrypoints second. |
| What adjacent real-world ESP32 experience should influence execution behavior? | [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md) | External empirical input, not a repo-grounded architecture claim. |
| When is the dossier complete enough to freeze? | [07-freeze-checklist.md](./07-freeze-checklist.md) | Use this before implementation begins. |

## Current freeze posture

This dossier began as a freeze candidate and now also includes post-freeze Phase 0 evidence, including the later multi-turn comms extension on real hardware. Phase 1 should use [10-phase-0-live-findings.md](./10-phase-0-live-findings.md) and the updated matrix rows before changing architecture.

## Current repo grounding for the dossier itself

| ID | State | Claim | Evidence |
| --- | --- | --- | --- |
| DOSSIER-001 | `CODE_GROUNDED` | Public docs navigation is controlled by `docs/docs.json`, and the current public architecture pages live under `docs/reference/*`. | [docs/docs.json#L23-L141](../../../docs/docs.json#L23-L141) |
| DOSSIER-002 | `CODE_GROUNDED` | Internal architecture notes already live under `docs/architecture/*`, so placing this dossier under `docs/architecture/embedded-esp32/` follows the existing internal-docs pattern instead of introducing public-docs-first pressure. | [docs/architecture/meerkat-runtime-dogma.md](../../../docs/architecture/meerkat-runtime-dogma.md), [docs/reference/design-philosophy.mdx#L19-L48](../../../docs/reference/design-philosophy.mdx#L19-L48) |
| DOSSIER-003 | `CODE_GROUNDED` | Example naming and layout are standardized, and shell-driven examples are first-class citizens in the examples tree. | [examples/README.md#L202-L217](../../../examples/README.md#L202-L217) |
