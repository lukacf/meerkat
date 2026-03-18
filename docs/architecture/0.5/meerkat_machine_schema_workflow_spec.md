# Meerkat 0.5 Machine Authority Workflow

Status: normative `0.5` workflow spec

## Purpose

This document defines the concrete workflow that connects:

- Rust-native machine and composition authority
- generated TLA+ and spec artifacts
- generated Rust kernels
- repo verification and drift checks

It turns the formalization strategy into a concrete in-repo authority path.

## Final Authority Model

The steady-state `0.5` model is:

- canonical machine authority is authored in
  `meerkat-machine-schema/src/catalog/*.rs`
- canonical composition authority is authored in
  `meerkat-machine-schema/src/catalog/compositions.rs`
- generated artifacts are checked in under:
  - `specs/machines/<machine>/`
  - `specs/compositions/<bundle>/`
- `xtask` is the only supported repo entrypoint for generation, verification,
  and drift detection

There is no release-authoritative external schema file. There is also no final
handwritten-kernel exception path for canonical machines.

## Final Repo Layout

```text
docs/architecture/0.5/
  meerkat_0_5_architecture_outline.md
  meerkat_0_5_execution_plan.md
  meerkat_0_5_implementation_plan.md
  meerkat_host_mode_cutover_spec.md
  meerkat_ops_lifecycle_seam_spec.md
  meerkat_surface_cutover_matrix.md
  meerkat_machine_formalization_strategy.md
  meerkat_machine_schema_workflow_spec.md

meerkat-machine-schema/
  src/catalog/*.rs
  src/catalog/compositions.rs

meerkat-machine-codegen/
  src/render.rs

xtask/
  src/machines.rs

specs/machines/<machine>/
  contract.md
  model.tla
  ci.cfg
  deep.cfg
  mapping.md

specs/compositions/<bundle>/
  contract.md
  model.tla
  ci.cfg
  deep.cfg
  mapping.md

meerkat-machine-kernels/
  src/generated/*.rs
```

## Artifact Rules

### Required for every canonical machine

Every canonical machine must have:

- a Rust-native catalog definition
- `contract.md`
- `model.tla`
- `ci.cfg`
- `deep.cfg`
- `mapping.md`
- a generated Rust kernel module under `meerkat-machine-kernels/src/generated/`

### Required for every canonical composition bundle

Every canonical composition must have:

- a Rust-native catalog definition
- `contract.md`
- `model.tla`
- `ci.cfg`
- `deep.cfg`
- `mapping.md`
- witness configs when the composition declares bounded witness paths

### Interpretive rule

- Rust-native catalog definitions are the semantic source of truth
- checked-in spec artifacts are generated or mechanically refreshed from that
  authority
- `mapping.md` may retain hand-authored explanatory text, but its generated
  coverage block is authoritative for item inventory

## Implementation Status Model

The only acceptable final implementation status for canonical machines is:

- `SchemaKernel`

The following are execution statuses only:

- `SchemaExtension`
- `BoundaryRedesign`

They may exist while landing `0.5`, but they are not valid end states.

## Workflow

1. Define or update the canonical machine/composition in the Rust catalog.
2. Run:

```bash
cargo xtask machine-codegen --machine <name>
cargo xtask machine-check-drift --machine <name>
```

or:

```bash
cargo xtask machine-codegen --composition <bundle>
cargo xtask machine-check-drift --composition <bundle>
```

3. Run bounded verification:

```bash
cargo xtask machine-verify --machine <name>
cargo xtask machine-verify --composition <bundle>
```

4. For richer local validation, run:

```bash
cargo xtask machine-verify --profile deep --machine <name>
cargo xtask machine-verify --profile deep --composition <bundle>
```

5. Land the authority, generated artifacts, and any required surrounding
   documentation in the same change.

## Code Generation Contract

`meerkat-machine-codegen` is responsible for rendering:

- machine authority modules
- composition authority modules
- generated coverage blocks for `mapping.md`
- additional generated contract/model/profile artifacts as the codegen surface
  expands

`xtask` is responsible for:

- selecting canonical machines and compositions
- validating the catalog
- validating compositions against their machine set
- writing generated outputs into `specs/`
- checking drift
- running TLC where checked-in verification profiles exist

## Verification Contract

The minimum required repo commands are:

```bash
cargo xtask machine-codegen --all
cargo xtask machine-check-drift --all
cargo xtask machine-verify --all
```

The required behavior is:

- drift fails if generated artifacts are stale or missing
- drift fails if docs/RCT/spec files still describe the obsolete authority
  model
- drift fails if required mapping coverage is missing
- verification fails if canonical machines/compositions do not validate or if
  TLC fails on checked-in bounded profiles

## Mapping Coverage Rule

Every canonical machine and composition must carry structured coverage in the
Rust catalog:

- code anchors pointing at current implementation seams, tests, or examples
- scenario coverage naming target behaviors that must survive the `0.5`
  refactor

That coverage is used to refresh the generated portion of `mapping.md` and to
fail drift checks when anchors disappear or authoritative items are no longer
accounted for.

## Release Rule

`0.5` is not complete until:

- all canonical machines and composition bundles are catalog-owned
- all checked-in semantic artifacts are aligned to that authority
- no authoritative doc or RCT file still assumes a release-authoritative
  external schema file
- no canonical machine remains in `SchemaExtension` or `BoundaryRedesign`
