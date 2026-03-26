# Meerkat 0.5 Composition Specs

This directory is the checked-in formal composition bundle for the `0.5`
machine-authority package.

Companion workflow docs:

- `docs/architecture/0.5/meerkat_machine_formalization_strategy.md`
- `docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md`
- `docs/architecture/0.5/meerkat_0_5_execution_plan.md`

Each canonical composition bundle lives under its own directory:

- `contract.md`
- `model.tla`
- `ci.cfg`
- `deep.cfg`
- `mapping.md`
- witness configs when the bundle declares bounded witness paths

Status:

- canonical composition authority is authored in
  `meerkat-machine-schema/src/catalog/compositions.rs`
- checked-in composition artifacts under `specs/compositions/` are refreshed
  from that authority
- `mapping.md` may retain hand-authored explanatory text, but its generated
  coverage block is authoritative for route, scheduler-rule, and invariant
  inventory

Validation:

- `make machine-codegen`
- `make machine-check-drift`
- `make machine-verify`
- `cargo xtask machine-codegen --all`
- `cargo xtask machine-check-drift --all`
- `cargo xtask machine-verify --all`

When the workspace is busy, prefer the `make machine-*` targets. They build
`xtask` into an isolated target dir and then run the binary directly, which
avoids confusing `cargo run` lock waits.

Canonical composition set:

- `runtime_pipeline`
- `peer_runtime_bundle`
- `ops_runtime_bundle`
- `surface_event_runtime_bundle`
- `continuation_runtime_bundle`
- `external_tool_bundle`
- `mob_bundle`
