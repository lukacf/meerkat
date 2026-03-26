# Meerkat 0.5 Machine Specs

This directory is the checked-in formal machine-spec bundle for the `0.5`
architecture package.

Companion workflow docs:

- `docs/architecture/0.5/meerkat_machine_formalization_strategy.md`
- `docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md`
- `docs/architecture/0.5/meerkat_0_5_execution_plan.md`

Each canonical machine lives under its own directory:

- `contract.md`
- `model.tla`
- `ci.cfg`
- `deep.cfg`
- `mapping.md`
- generated Rust kernel at `meerkat-machine-kernels/src/generated/<machine>.rs`

Status:

- the Rust-native catalog under `meerkat-machine-schema/src/catalog/*.rs` is
  the long-term semantic authority for the `0.5` target
- the checked-in machine artifacts in `specs/machines/` are generated,
  reviewable proof artifacts derived from that authority
- where current implementation diverges, `mapping.md` calls that out explicitly
- the checked-in `ci.cfg` files are the bounded CI profiles for TLC

Validation:

- `make machine-check-drift`
- `make machine-verify`
- `cargo xtask machine-verify --all`
- `./specs/machines/validate.sh` (thin wrapper over the xtask command above)
- or per machine:
  `tlc -metadir specs/machines/.tlc/<machine> -config specs/machines/<machine>/ci.cfg specs/machines/<machine>/model.tla`

When the workspace is busy, prefer the `make machine-*` targets. They build
`xtask` into an isolated target dir and then run the binary directly, which
avoids confusing `cargo run` lock waits.

Canonical machine set:

- `runtime_ingress`
- `runtime_control`
- `input_lifecycle`
- `peer_comms`
- `external_tool_surface`
- `turn_execution`
- `ops_lifecycle`
- `mob_orchestrator`
- `mob_lifecycle`
- `flow_run`

Final release-closure inventory:

All canonical `0.5` machines now target the same final implementation mode:
`SchemaKernel`. The checked-in Rust catalog remains the semantic authority and
the generated kernel is the explicit transition boundary consumed by repo
verification.

| Machine | Final mode | Catalog authority | Generated transition boundary |
| --- | --- | --- | --- |
| `input_lifecycle` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/input_lifecycle.rs` | `meerkat-machine-kernels/src/generated/input_lifecycle.rs` |
| `runtime_ingress` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/runtime_ingress.rs` | `meerkat-machine-kernels/src/generated/runtime_ingress.rs` |
| `runtime_control` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/runtime_control.rs` | `meerkat-machine-kernels/src/generated/runtime_control.rs` |
| `mob_lifecycle` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/mob_lifecycle.rs` | `meerkat-machine-kernels/src/generated/mob_lifecycle.rs` |
| `ops_lifecycle` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/ops_lifecycle.rs` | `meerkat-machine-kernels/src/generated/ops_lifecycle.rs` |
| `peer_comms` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/peer_comms.rs` | `meerkat-machine-kernels/src/generated/peer_comms.rs` |
| `external_tool_surface` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/external_tool_surface.rs` | `meerkat-machine-kernels/src/generated/external_tool_surface.rs` |
| `turn_execution` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/turn_execution.rs` | `meerkat-machine-kernels/src/generated/turn_execution.rs` |
| `flow_run` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/flow_run.rs` | `meerkat-machine-kernels/src/generated/flow_run.rs` |
| `mob_orchestrator` | `SchemaKernel` | `meerkat-machine-schema/src/catalog/mob_orchestrator.rs` | `meerkat-machine-kernels/src/generated/mob_orchestrator.rs` |

Verification posture:

- TLA+ models are model-checked and evolve with the implementation
- Rust-native authority in `meerkat-machine-schema/src/catalog/*.rs` drives the
  generated machine bundle
- `xtask` is the authoritative generation/drift/verification entrypoint;
  `validate.sh` is now only a convenience wrapper over that path

Current bounded TLC snapshot:

- `runtime_ingress`: `1,007` distinct states
- `runtime_control`: `90`
- `input_lifecycle`: `25`
- `peer_comms`: `119`
- `external_tool_surface`: `123`
- `turn_execution`: `674`
- `ops_lifecycle`: `335`
- `mob_orchestrator`: `143`
- `mob_lifecycle`: `62`
- `flow_run`: `16`
