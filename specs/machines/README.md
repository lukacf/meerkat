# Meerkat 0.6 Machine Specs

This directory is the canonical executable machine-spec home for the two-kernel
`0.6` architecture.

Each machine directory contains:

- `contract.md`
- `model.tla`
- `ci.cfg`
- `deep.cfg`
- `mapping.md`
- optional focused liveness or audit configs when the machine has additional
  proof lanes

Canonical machine set:

- `meerkat_machine`
- `mob_machine`
- perimeter machines that remain outside the kernels:
  - `occurrence_lifecycle`
  - `schedule_lifecycle`

Status:

- `specs/machines/` is the canonical executable spec home
- the kernel authorities live at:
  - `specs/machines/meerkat_machine/`
  - `specs/machines/mob_machine/`
- the schema catalog and generated authority artifacts must match these specs
- where implementation or catalog coverage diverges, `mapping.md` calls that
  out explicitly
- the checked-in `ci.cfg` files are the bounded CI TLC profiles

Validation:

- `make machine-codegen`
- `make machine-check-drift`
- `make machine-verify`
- `cargo xtask machine-verify --all`
- `./specs/machines/validate.sh`
- or per machine:
  `tlc -metadir specs/machines/.tlc/<machine> -config specs/machines/<machine>/ci.cfg specs/machines/<machine>/model.tla`

When the workspace is busy, prefer the `make machine-*` targets. They build
`xtask` into an isolated target dir and then run the binary directly.
