# Source Of Truth

This RCT package is derived from the normative `0.5` architecture docs already
checked into `docs/architecture/0.5/`, plus the now-validated target
machine/composition authority under `meerkat-machine-schema` and
`specs/{machines,compositions}`.

It does not supersede them during planning and migration. It turns them into
an executable requirement and phase model for the implementation cutover. The
Rust-native machine and composition catalog is already the semantic source of
truth for the `0.5` target.

## Normative Inputs

| Document | What it governs | How this RCT package uses it |
| --- | --- | --- |
| `meerkat_0_5_architecture_outline.md` | target architecture, ownership model, machine census, runtime semantics, surface posture | source for `REQ`, `TYPE`, `INV`, and high-level `CHOKE` families |
| `meerkat_0_5_execution_plan.md` | phase-level rollout, definition of done, release gates | source for legacy phase crosswalks and global release rules |
| `meerkat_0_5_implementation_plan.md` | crate/file ownership, task decomposition, verification/deletion checkpoints | source for task granularity, touched files, and checklist-level closure conditions |
| `meerkat_host_mode_cutover_spec.md` | host-mode replacement contract and behavior preservation | source for peer/runtime bridge requirements, host-mode E2Es, and cutover chokepoints |
| `meerkat_ops_lifecycle_seam_spec.md` | shared async-operation lifecycle seam | source for lifecycle types, registry contracts, and mob/background-op convergence tasks |
| `meerkat_surface_cutover_matrix.md` | surface-specific bypass deletion and release gates | source for per-surface RCT phases, contracts, and E2Es |
| `meerkat_machine_formalization_strategy.md` | machine implementation modes and honesty rules | source for `SchemaKernel` / `SchemaExtension` / `BoundaryRedesign` requirements and verification gates |
| `meerkat_machine_schema_workflow_spec.md` | machine artifact layout, Rust-catalog/codegen workflow, CI gates | source for tooling contracts and machine-authority validation |
| `meerkat_sm_nomenclature.md` | canonical naming and machine/non-machine vocabulary | source for term normalization and type names |
| `meerkat-machine-schema/src/catalog/` | validated target machine/composition authority | source for cutover target semantics and generated spec truth |
| `specs/machines/` + `specs/compositions/` | generated TLA/spec bundles and validation configs | source for target-model proof artifacts used by the cutover program |

## Machine Artifact Home

The formal machine bundle now lives in-repo:

- `specs/machines/`
- `specs/compositions/`

This package treats the in-repo generated bundles as the reviewable proof
artifacts for the target model during implementation cutover.

## RCT Package Authority

Within `docs/architecture/0.5/rct/`:

- `spec.yaml` is authoritative for requirement IDs and inventory
- `plan.yaml` is authoritative for RCT phase numbering, dependencies, and
  build-first gate ordering
- `checklist.yaml` is authoritative for task decomposition and task status
- `gaps-and-contradictions.md` records how formerly-open package-level
  contradictions were resolved

Long-term semantic authority for the target is:

- Rust-native machine and composition authority under
  `meerkat-machine-schema/src/catalog/`
- CI verification that rejects drift between machine authority and downstream
  code/docs

## Promotion Rule

When this package is promoted into a repo-root `.rct/` workspace:

1. keep `spec.yaml`, `plan.yaml`, and `checklist.yaml` as the carried-forward
   sources of truth
2. scaffold automation around them rather than re-deriving them
3. use the repo-local scaffold and loop assets under
   `.claude/skills/rct-methodology/`
4. preserve the repo-specific gate policy:
   - build gate first
   - reviewer gates second
   - no cargo build/check/test inside reviewer gates
