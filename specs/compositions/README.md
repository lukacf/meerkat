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

Canonical composition set:

- `meerkat_mob_seam`
- perimeter compositions retained after audit:
  - `schedule_bundle`
  - `schedule_runtime_bundle`
  - `schedule_mob_bundle`

Status:

- internal routes inside `MeerkatMachine` and `MobMachine` are no longer
  modeled as inter-machine compositions
- `meerkat_mob_seam` is the sole inter-kernel composition
- the retained perimeter compositions were audited during the two-kernel
  collapse:
  - `schedule_bundle` remains the pure schedule/occurrence perimeter bundle
  - `schedule_runtime_bundle` remains because it references only
    schedule/occurrence delivery protocol edges into the runtime perimeter
  - `schedule_mob_bundle` remains because it references only
    schedule/occurrence delivery protocol edges into the mob perimeter

Validation:

- `make machine-codegen`
- `make machine-check-drift`
- `make machine-verify`
- `cargo xtask machine-codegen --all`
- `cargo xtask machine-check-drift --all`
- `cargo xtask machine-verify --all`
