# Embedded ESP32 Planning Dossier

This directory is the canonical internal planning dossier for the ESP32-S3 embedded Meerkat program.

`docs/docs.json` intentionally remains unchanged. The material here is internal planning authority, not public Mintlify navigation, and must stay decision-complete enough for a subsequent autonomous implementation run to execute sequentially from Phase 0 through final closure without making architecture decisions on the fly.

## Dossier rules

1. Every current-state claim is either `CODE_GROUNDED` with direct repo evidence or `HYPOTHESIS` with a Phase 0 live contract test that can prove or reject it.
2. Embedded is a surface and backend set, not a shadow runtime model.
3. The canonical semantic path stays runtime-backed: `Surface -> RuntimeSessionAdapter -> SessionService -> AgentFactory::build_agent()`.
4. Capability limits are profile limits and backend limits, not semantic forks.
5. Phase 0 is hardware-first on a real ESP32-S3 board. Simulator evidence is useful but cannot close critical assumptions by itself.
6. There are no core-scope deferrals. If a phase exposes a blocker, the plan must absorb it immediately instead of pushing it to a backlog.

## Reader order

1. [01-code-grounded-baseline.md](./01-code-grounded-baseline.md)
2. [02-external-contract-matrix.md](./02-external-contract-matrix.md)
3. [03-requirements-and-rtm.md](./03-requirements-and-rtm.md)
4. [04-implementation-phase-plan.md](./04-implementation-phase-plan.md)
5. [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md)
6. [06-example-smoke-specs.md](./06-example-smoke-specs.md)
7. [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md)
8. [07-freeze-checklist.md](./07-freeze-checklist.md)

## Authority map

| Question | Authority | Notes |
| --- | --- | --- |
| What does the current codebase already guarantee? | [01-code-grounded-baseline.md](./01-code-grounded-baseline.md) | All rows in this file are `CODE_GROUNDED`. |
| Which external assumptions can sink the project and how do we test them? | [02-external-contract-matrix.md](./02-external-contract-matrix.md) | All assumption rows are `HYPOTHESIS` until Phase 0 evidence updates them. |
| What are the immutable invariants, contracts, requirements, E2E obligations, and choke points? | [03-requirements-and-rtm.md](./03-requirements-and-rtm.md) | This is the requirement-trace authority. |
| What is the sequential implementation order and the gate for each phase? | [04-implementation-phase-plan.md](./04-implementation-phase-plan.md) | This file is requirement-driven, not component-driven. |
| What should an autonomous coding agent do phase by phase? | [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md) | This is the execution choreography. |
| What exactly must `examples/036` and `examples/037` teach and prove? | [06-example-smoke-specs.md](./06-example-smoke-specs.md) | These examples are user-facing patterns first and smoke entrypoints second. |
| What adjacent real-world ESP32 experience should influence execution behavior? | [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md) | External empirical input, not a repo-grounded architecture claim. |
| When is the dossier complete enough to freeze? | [07-freeze-checklist.md](./07-freeze-checklist.md) | Use this before implementation begins. |

## Current freeze posture

This dossier is authored as a freeze candidate. A phase run should not begin until [07-freeze-checklist.md](./07-freeze-checklist.md) passes and the team can start Phase 0 immediately on real hardware.

## Current repo grounding for the dossier itself

| ID | State | Claim | Evidence |
| --- | --- | --- | --- |
| DOSSIER-001 | `CODE_GROUNDED` | Public docs navigation is controlled by `docs/docs.json`, and the current public architecture pages live under `docs/reference/*`. | [docs/docs.json#L23-L141](../../../docs/docs.json#L23-L141) |
| DOSSIER-002 | `CODE_GROUNDED` | Internal architecture notes already live under `docs/architecture/*`, so placing this dossier under `docs/architecture/embedded-esp32/` follows the existing internal-docs pattern instead of introducing public-docs-first pressure. | [docs/architecture/meerkat-runtime-dogma.md](../../../docs/architecture/meerkat-runtime-dogma.md), [docs/reference/design-philosophy.mdx#L19-L48](../../../docs/reference/design-philosophy.mdx#L19-L48) |
| DOSSIER-003 | `CODE_GROUNDED` | Example naming and layout are standardized, and shell-driven examples are first-class citizens in the examples tree. | [examples/README.md#L202-L217](../../../examples/README.md#L202-L217) |
