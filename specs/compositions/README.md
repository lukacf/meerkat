# Meerkat 0.6 Composition Specs

This directory is the canonical executable composition-spec home for the
two-kernel `0.6` architecture.

Each composition directory contains:

- `contract.md`
- `model.tla`
- `ci.cfg`
- `deep.cfg`
- `mapping.md`
- optional witness or liveness configs when the composition has additional
  proof lanes

Status:

- `specs/compositions/` is the canonical executable composition-spec home
- the schema catalog and generated authority artifacts define the canonical
  composition roster
- internal routes inside `MeerkatMachine` and `MobMachine` are not modeled as
  inter-machine compositions
- the retained perimeter/workgraph compositions were audited during the two-kernel
  collapse:
  - `auth_lease_bundle` remains because it publishes auth lease lifecycle
    facts across the auth authority perimeter
  - `schedule_bundle` remains the pure schedule/occurrence perimeter bundle
  - `schedule_runtime_bundle` remains because it references only
    schedule/occurrence delivery protocol edges into the runtime perimeter
  - `schedule_mob_bundle` remains because it references only
    schedule/occurrence delivery protocol edges into the mob perimeter
  - `workgraph_attention_bundle` remains because WorkGraph item lifecycle and
    attention binding lifecycle are separate WorkGraph-owned authority surfaces

Reading `model.tla`:

- `UnchangedFrame_<16 hex>` operators are generated state frames. Each distinct
  `UNCHANGED << ... >>` tuple is defined exactly once, immediately after
  `vars == << ... >>`, and every action that leaves those variables unchanged
  references it by name. The suffix is an FNV-1a 64 hash of the frame body, so
  a frame keeps its name across unrelated schema edits.
- A frame wider than 96 variables is emitted as a conjunction of 64-variable
  tuples inside its definition. That split works around a TLC semantic-pass
  overflow and is the same formula as one wide tuple.
- Frames are owned by the renderer (`meerkat-machine-codegen`); never edit them
  by hand.

Validation:

Generation and drift checks, without running TLC:

- `make machine-codegen` regenerates the authority artifacts.
- `make machine-check-drift` checks that generated artifacts match their sources.
- Direct xtask alternatives are `./scripts/repo-cargo xtask machine-codegen --all`
  and `./scripts/repo-cargo xtask machine-check-drift --all`.

For TLC verification, `tlc` must be on `PATH`:

- `make machine-verify` is the normal budgeted TLC lane. It retains drift and
  structural checks, runs the bounded adaptive layer-terminal witness
  (`specs/compositions/adaptive_mob_bundle/witness-layer_terminal_feedback.cfg`),
  and excludes the full `meerkat_mob_seam` and `adaptive_mob_bundle` composition
  sweeps. Its wrapper also skips Cargo-backed post-checks.
- `make machine-verify-full` is the expensive, on-demand full catalog sweep with
  the default `Ci` (`ci.cfg`) profile and no composition exclusions. It can take
  hours; "full" does not select the `Deep` profile or an unbounded state space.
- `./scripts/repo-cargo xtask machine-verify --all` also selects the full sweep
  unless callers supply overriding flags; it does not select the budgeted
  wrapper.
