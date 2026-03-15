# Meerkat 0.5 RCT Package

This directory is the docs-local `.rct` package for the full Meerkat `0.5`
implementation program.

It is intentionally stored under `docs/architecture/0.5/rct/` instead of the
repo root so the `0.5` planning package stays colocated with the architecture
docs it operationalizes.

## Scope

This package does four things:

1. turns the normative `0.5` architecture docs into a requirement-traced RCT
   specification
2. normalizes the phase/workstream contradictions between the execution and
   implementation plans
3. records the resolved semantic decisions and their execution consequences
   instead of burying them in task prose
4. provides the spec/plan/checklist artifacts needed to run a full RCT program
   manually or to promote later into a repo-root `.rct/` workspace

## Files

- `source-of-truth.md`
  maps the authoritative `0.5` docs and explains which document governs which
  domain
- `rct-theory.md`
  local condensed copy of the RCT rules this package follows
- `spec.yaml`
  authoritative requirement inventory for the `0.5` implementation
- `plan.yaml`
  normalized RCT phase plan with dependencies and crosswalks back to the
  current `0.5` plans
- `checklist.yaml`
  atomic task checklist with phase-local verification commands and gate data
- `traceability.md`
  initial requirement traceability matrix
- `typed-but-unwired.md`
  carry-forward list for types/contracts already described by docs but not yet
  fully wired in runtime code
- `gaps-and-contradictions.md`
  resolved contradiction ledger and decision-closure record for this package
- `reviewer-manifest.yaml`
  reviewer IDs and prompt sources for a manual `rct-lite` execution mode
- `blockers.yaml`
  empty initial blocker ledger

## Deliberate Omissions

This is a docs-only planning package, not a live Luka Loop workspace yet.

It intentionally omits:

- `scripts/`
- `prompts/`
- `outputs/`
- copied reviewer prompt files under `agents/`

If the team later wants to run the automated Luka Loop, those can be scaffolded
from the `rct-development` skill and pointed at the already-authored
`spec.yaml`, `plan.yaml`, and `checklist.yaml`.

## Authority Rule

Within this folder:

- `spec.yaml` is the authoritative inventory of what `0.5` must implement
- `plan.yaml` is the authoritative phase model for RCT execution
- `checklist.yaml` is the authoritative task-status source of truth

These files do **not** replace the normative architecture docs during
planning/migration. They trace and operationalize them. Once a canonical
machine lands in checked-in schema or explicit Rust kernel form, that
executable machine definition becomes the long-term semantic source of truth
for that machine. See `source-of-truth.md`.

## Normalization Rule

Where the `0.5` docs disagreed or left sequencing ambiguous, this package
records the final package stance and propagates it into the requirement,
phase, and checklist layers.

That closure ledger lives in `gaps-and-contradictions.md`.
