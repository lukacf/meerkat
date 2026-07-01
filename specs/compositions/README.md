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

Validation:

- `make machine-codegen`
- `make machine-check-drift`
- `make machine-verify`
- `cargo xtask machine-codegen --all`
- `cargo xtask machine-check-drift --all`
- `cargo xtask machine-verify --all`
