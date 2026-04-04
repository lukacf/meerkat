# Consistency Review And Freeze Checklist

Use this checklist before treating the dossier as frozen and before starting the autonomous implementation run.

## Review commands

These commands are cheap preflight checks for the dossier itself:

```bash
rg -n "CODE_GROUNDED|HYPOTHESIS" docs/architecture/embedded-esp32
rg -n "REQ-|CONTRACT-|INV-|E2E-|CHOKE-" docs/architecture/embedded-esp32
rg -n "UNEXECUTED" docs/architecture/embedded-esp32
rg -n "shadow implementation|shadow runtime|shadow loop|shadow session" docs/architecture/embedded-esp32
rg -n "TBD|future work|later" docs/architecture/embedded-esp32
rg -n "redeploy|OTA|topology report|visualizer|operator workflow" docs/architecture/embedded-esp32
rg -n "ASSUMP-010|ASSUMP-012|CHOKE-005|metric-position|topology-only fallback|host-core-check|ed25519-dalek|fallback candidate" docs/architecture/embedded-esp32
```

## Freeze checklist

| Check ID | Pass condition | How to check |
| --- | --- | --- |
| FREEZE-001 | Every current-state claim in the dossier is explicitly `CODE_GROUNDED` and includes direct repo evidence. | Review [01-code-grounded-baseline.md](./01-code-grounded-baseline.md) and spot-check evidence links. |
| FREEZE-002 | Every external assumption is explicitly `HYPOTHESIS` and has a real phase-owned live test path. | Review [02-external-contract-matrix.md](./02-external-contract-matrix.md). |
| FREEZE-002A | Known platform and toolchain constraints that are already verified are recorded outside the matrix and do not masquerade as unresolved hypotheses. | Review [09-technical-dependency-analysis.md](./09-technical-dependency-analysis.md). |
| FREEZE-003 | No critical planning decision remains open. | Review the decision-closure section in [02-external-contract-matrix.md](./02-external-contract-matrix.md). |
| FREEZE-004 | Every `MUST` row has an owning phase, a runtime caller, and a proof artifact. | Review [03-requirements-and-rtm.md](./03-requirements-and-rtm.md). |
| FREEZE-005 | Every phase has a behavioral done-when, a real-entrypoint proof, and a negative control. | Review [04-implementation-phase-plan.md](./04-implementation-phase-plan.md). |
| FREEZE-006 | Every autonomous phase packet contains goal, inputs, tasks, TDD order, commands, evidence, blockers, reviewers, and handoff. | Review [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md). |
| FREEZE-007 | Both example deliverables are fully specified as real entrypoints and smoke tests. | Review [06-example-smoke-specs.md](./06-example-smoke-specs.md). |
| FREEZE-008 | No required-scope item is described as “do this afterward”, “future work”, or `TBD`. | Run the grep above and inspect any hits. The checklist command and this checklist row are expected matches; other hits require review. |
| FREEZE-009 | The dossier does not normalize a shadow implementation or a browser-style substrate-authority path for embedded. | Review `INV-001`, `INV-002`, `DEC-001`, and `DEC-003`. |
| FREEZE-010 | `docs/docs.json` remains unchanged and the dossier is still internal-only. | Inspect [docs/docs.json#L23-L141](../../../docs/docs.json#L23-L141). |
| FREEZE-011 | Phase 0 can start immediately with known required inputs and without opening new product decisions. | Review [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md) Phase 0 packet and confirm hardware/tooling availability. |
| FREEZE-012 | The RTM and matrix are ready to be updated in-place by subsequent implementation phases. | Spot-check cross-links across [02](./02-external-contract-matrix.md), [03](./03-requirements-and-rtm.md), and [05](./05-autonomous-execution-pack.md). |
| FREEZE-013 | Field learnings from [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md) are reflected in the execution packets and example specs, not isolated in an appendix. | Cross-check [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md) and [06-example-smoke-specs.md](./06-example-smoke-specs.md) against [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md). |
| FREEZE-014 | Public examples remain valuable operator-facing patterns with real run modes and user-facing outputs, not only smoke harnesses. | Review [06-example-smoke-specs.md](./06-example-smoke-specs.md) and confirm Example 036 carries redeploy workflow expectations and Example 037 carries a topology report or visualizer requirement. |
| FREEZE-015 | No invariant, contract, or swarm-specific choke point is orphaned from the phase plan. | Cross-check [03-requirements-and-rtm.md](./03-requirements-and-rtm.md) against [04-implementation-phase-plan.md](./04-implementation-phase-plan.md). |
| FREEZE-016 | Phase 0 is explicitly limited to single-node and Meerkat-model verification, while swarm viability is owned by Phase 3 and Example 037. | Review [02-external-contract-matrix.md](./02-external-contract-matrix.md), [04-implementation-phase-plan.md](./04-implementation-phase-plan.md), and [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md). |
| FREEZE-017 | Example 037 has a predeclared topology-only fallback and does not require an autonomous agent to invent a new product decision if metric positioning is rejected. | Review [02-external-contract-matrix.md](./02-external-contract-matrix.md), [03-requirements-and-rtm.md](./03-requirements-and-rtm.md), and [06-example-smoke-specs.md](./06-example-smoke-specs.md). |
| FREEZE-018 | Example 037 also has an explicit hard-stop rule if peer discovery or peer messaging is rejected on real hardware, so only metric positioning can fall back. | Review [02-external-contract-matrix.md](./02-external-contract-matrix.md), [04-implementation-phase-plan.md](./04-implementation-phase-plan.md), [05-autonomous-execution-pack.md](./05-autonomous-execution-pack.md), and [06-example-smoke-specs.md](./06-example-smoke-specs.md). |

## Freeze decision rule

The dossier is freeze-ready only when every checklist row above passes.

If any row fails:

- do not start the autonomous implementation run;
- update the dossier first;
- rerun the checklist before proceeding.

## Freeze record template

Fill this in when the branch is actually frozen for implementation:

| Field | Value |
| --- | --- |
| Freeze date | |
| Branch | |
| Reviewer or reviewer-agent set | |
| Phase 0 hardware environment confirmed | `yes` / `no` |
| Open blockers | |
| RTM status | |
| Matrix status | |
| Freeze verdict | `ready` / `not ready` |
