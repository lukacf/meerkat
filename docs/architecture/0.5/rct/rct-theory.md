# RCT Theory For Meerkat 0.5

This is the local methodology note for the `0.5` RCT package.

It is a condensed working copy of the `rct-development` rules, adapted for the
docs-local package and the repo's build-first Luka Loop policy.

## Core Rule

Every requirement must be traced from:

1. requirement inventory
2. phase assignment
3. atomic checklist task
4. verification command
5. evidence bundle

If a requirement cannot be traced through that chain, it is not ready to
implement.

## Gate Model

- Gate 0: external boundary validation
- Gate 1: target definition written and fully wired, red only at behavior level
- Gate 2: behavior implementation turning integration green
- Gate 3: full release closure, no stubs or deferred MUST scope

## Repo-Specific Gate Ordering

For this repo's implementation cutover, gate ordering is:

1. quick compile sanity check
2. build/test/phase verification gate
3. independent reviewer gates

Reviewer gates may inspect files, generated artifacts, logs, and test output,
but they may not run cargo build/check/test themselves. This avoids cargo-lock
contention and keeps reviewer work focused on semantic and integration review.

## Planning Rules

### Requirements first

Phases are derived from requirements, not from components.

Bad:

- implement scheduler
- implement dispatcher

Good:

- wire explicit `OperationInput` admission into runtime
- prove control-plane preemption beats ordinary ingress
- prove `event/push` no longer owns a side execution path

### Done-when must be behavioral

Compilation-only closure is acceptable only for:

- type-definition tasks in Phase 0
- artifact-landing tasks whose entire point is file/layout existence

All behavioral tasks must close on observable runtime behavior.

### No proxy-only closure for MUST scope

MUST/REQUIRED requirements need:

- direct command evidence
- real-entrypoint proof
- `VALIDATED` status in the traceability matrix

Harness-only evidence is `PROVISIONAL` and cannot close MUST scope.

## Required Tracking Artifacts

This package maintains:

- `spec.yaml`
- `plan.yaml`
- `checklist.yaml`
- `traceability.md`
- `typed-but-unwired.md`
- `blockers.yaml`

## Status Meanings

- `TYPED`
  type/schema/contract exists, runtime does not consume it yet
- `WIRED`
  runtime consumes it, but there is no real-entrypoint proof yet
- `VALIDATED`
  proven through the shipped runtime path with command evidence
- `PROVISIONAL`
  only proxy or harness evidence exists
- `MISSING`
- `DEFERRED`
- `STUBBED`

## Reality Gate

Every implementation phase must include at least one real-entrypoint test for
the shipped path relevant to that phase:

- CLI
- REST
- JSON-RPC
- MCP
- WASM/browser
- repo verification commands such as `cargo xtask machine-verify --all`

## Review Discipline

The reviewer set for this package follows `rct-development`:

- `spec-auditor`
- `integration-sheriff`
- `rct-guardian`
- `performance-gate`

The final closure gate may additionally include:

- `methodology-integrity`

Reviewers must discover state independently.
They should never be fed claimed results as ground truth.
They also should not re-run cargo build/check/test work that already belongs to
the build gate.

## Evidence Bundle Rule

Every gated phase should record:

- exact command
- commit SHA
- timestamp
- raw output excerpt or artifact path

This docs package does not yet carry a live `outputs/` tree, but the plan and
checklist assume those bundles will be produced during execution.

## Typed-But-Unwired Carry Forward

The `typed-but-unwired.md` file is not optional.

It is the mechanism that prevents these failures:

- type added but never consumed by runtime
- compatibility facade exists but still owns semantics
- machine kernel boundary declared but not actually used by shell code

## Meerkat-Specific Adaptation

For `0.5`, the most important RCT risks are:

- landing generated runtime machine owners before runtime semantics are closed
- closing surface phases before the external-tool or host-mode contracts are
  actually settled
- treating machine formalization as tooling garnish instead of a release gate
- allowing docs-only concepts such as `OperationInput`, continuation, or typed
  notices to remain forever unwired
