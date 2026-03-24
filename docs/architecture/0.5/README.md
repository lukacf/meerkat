# Meerkat 0.5 Architecture Package

This folder is the non-RCT architecture package for Meerkat `0.5`.

The package is organized by role rather than by chronology:

- [meerkat_0_5_architecture_outline.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_0_5_architecture_outline.md)
  semantic architecture hub: machine boundaries, ownership, runtime model,
  surface posture
- [meerkat_0_5_execution_plan.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_0_5_execution_plan.md)
  phase-level rollout and release-gate plan
- [meerkat_0_5_implementation_plan.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_0_5_implementation_plan.md)
  concrete repo backlog: workstreams, epics, tasks, owner files, deletion
  checkpoints, verification gates
- [meerkat_host_mode_cutover_spec.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_host_mode_cutover_spec.md)
  host-mode/runtime cutover seam
- [meerkat_ops_lifecycle_seam_spec.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_ops_lifecycle_seam_spec.md)
  async-operation lifecycle seam for mob-backed child work and background ops
- [meerkat_surface_cutover_matrix.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_surface_cutover_matrix.md)
  public-surface cutover and deletion matrix
- [meerkat_machine_formalization_strategy.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_machine_formalization_strategy.md)
  machine truth / verification strategy
- [meerkat_machine_schema_workflow_spec.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md)
  Rust-native authority/catalog workflow and CI contract
- [meerkat_sm_nomenclature.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_sm_nomenclature.md)
  vocabulary and naming reference
- [meerkat_0_5_dogma.md](/Users/luka/src/meerkat/docs/architecture/0.5/meerkat_0_5_dogma.md)
  hardline doctrine for ownership, terminalization, projection, and surface
  truth

Ground-truth rule:

- during planning and migration, this package is the architecture source of
  truth for the branch
- once a canonical machine lands in the checked-in Rust-native authority
  catalog and generated-kernel path, that executable machine definition becomes the long-term semantic
  source of truth for that machine

Machine-spec home:

- the checked-in formal machine bundle now lives under `specs/machines/`
