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

Status:

- `specs/machines/` is the canonical executable spec home
- the schema catalog and generated authority artifacts define the canonical
  machine roster
- where implementation or catalog coverage diverges, `mapping.md` calls that
  out explicitly
- the checked-in `ci.cfg` files are the bounded CI TLC profiles

Reading `model.tla`:

- `UnchangedFrame_<16 hex>` operators are generated state frames. Each distinct
  `UNCHANGED << ... >>` tuple is defined exactly once, ahead of the transition
  actions, and every action that leaves those variables unchanged references it
  by name. The suffix is an FNV-1a 64 hash of the frame body, so a frame keeps
  its name across unrelated schema edits.
- `UNCHANGED vars` (the whole-state frame, used by `TerminalStutter`) stays
  inline; only per-field frames are named.
- Frames are owned by the renderer (`meerkat-machine-codegen`); never edit them
  by hand.

Validation:

Generation and drift checks, without running TLC:

- `make machine-codegen` regenerates the authority artifacts.
- `make machine-check-drift` checks that generated artifacts match their sources.

For TLC verification, `tlc` must be on `PATH`:

- `make machine-verify` is the normal budgeted TLC lane. It retains drift and
  structural checks, runs the bounded adaptive layer-terminal witness
  (`specs/compositions/adaptive_mob_bundle/witness-layer_terminal_feedback.cfg`),
  and excludes the full `meerkat_mob_seam` and `adaptive_mob_bundle` composition
  sweeps. Its wrapper also skips Cargo-backed post-checks.
- `make machine-verify-full` is the expensive, on-demand full catalog sweep with
  the default `Ci` (`ci.cfg`) profile and no composition exclusions. It can take
  hours; "full" does not select the `Deep` profile or an unbounded state space.
- `./scripts/repo-cargo xtask machine-verify --all` and
  `./specs/machines/validate.sh` also select the full sweep unless callers supply
  overriding flags; they do not select the budgeted wrapper.
- To check one machine:
  `tlc -metadir specs/machines/.tlc/<machine> -config specs/machines/<machine>/ci.cfg specs/machines/<machine>/model.tla`

When the workspace is busy, prefer the `make machine-*` targets. They build
`xtask` into an isolated target dir and then run the binary directly.
