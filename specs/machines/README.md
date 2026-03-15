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
- `mapping.md`

Status:

- these artifacts are normative for `0.5` machine semantics until a machine's
  checked-in schema or explicit Rust kernel becomes the long-term semantic
  authority
- where current implementation diverges, `mapping.md` calls that out explicitly
- the checked-in `ci.cfg` files are the bounded CI profiles for TLC

Validation:

- `./specs/machines/validate.sh`
- or per machine:
  `tlc -metadir specs/machines/.tlc/<machine> -config specs/machines/<machine>/ci.cfg specs/machines/<machine>/model.tla`

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

Verification posture:

- TLA+ models are model-checked and evolve with the implementation
- schema-kernel machines eventually add checked-in `schema.yaml` plus generated
  outputs under the same machine directory
- PureHand machines keep the TLA+ model plus explicit Rust kernel tests in the
  owning crate

Current bounded TLC snapshot:

- `runtime_ingress`: `12,127` distinct states
- `runtime_control`: `84`
- `input_lifecycle`: `17`
- `peer_comms`: `13,672`
- `external_tool_surface`: `7,056`
- `turn_execution`: `53`
- `ops_lifecycle`: `49,729`
- `mob_orchestrator`: `32`
- `mob_lifecycle`: `6`
- `flow_run`: `80`
