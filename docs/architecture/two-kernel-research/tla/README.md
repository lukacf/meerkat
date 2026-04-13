# Target TLA+ Pointers

The canonical executable TLA+ sources were promoted into `specs/` as part of
the two-kernel identity-first migration. This directory is now pointer-only.
No executable `.tla` or `.cfg` files live here anymore.

Canonical homes:

- `specs/machines/meerkat_machine/`
  - `model.tla`
  - `ci.cfg`
  - `deep.cfg`
- `specs/machines/mob_machine/`
  - `model.tla`
  - `ci.cfg`
  - `deep.cfg`
  - focused liveness/audit configs
- `specs/compositions/meerkat_mob_seam/`
  - `model.tla`
  - `ci.cfg`
  - `deep.cfg`
  - focused liveness configs

These are still the same proven models that backed the two-kernel research
freeze. The difference is only location and canonical ownership: `specs/` is
now the executable spec home, and this directory keeps documentation pointers
only so the research notes have a stable landing page.
