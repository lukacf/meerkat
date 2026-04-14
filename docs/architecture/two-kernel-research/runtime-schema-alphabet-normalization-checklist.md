# Runtime Schema Alphabet Normalization Checklist

This checklist tracks the exact-runtime-alphabet normalization needed to make the canonical machine schemas safe as the source for `schema -> codegen -> machine kernels`.

## Plan

### Implementation
- [x] Add `meerkat-machine-derive` with `#[derive(CommandManifest)]`
- [x] Derive manifests on all Meerkat runtime command enums
- [x] Derive manifest on `MobMachineCommand`
- [x] Export canonical Meerkat union manifest helper
- [x] Export canonical Mob manifest helper with the two test-only exclusions
- [x] Add `signals` to `MachineSchema`
- [x] Replace transition `on` matching with `TriggerMatch { kind, variant, bindings }`
- [x] Add explicit `RouteTarget.kind = Input | Signal`
- [x] Normalize `MeerkatMachine.inputs` to the exact runtime command set
- [x] Move all non-runtime Meerkat triggers into `MeerkatMachine.signals`
- [x] Normalize `MobMachine.inputs` to the exact runtime command set
- [x] Move all non-runtime Mob triggers into `MobMachine.signals`
- [x] Retarget `retire_member` entry input to `MobMachine.Retire`
- [x] Retarget `destroy_mob` entry input to `MobMachine.Destroy`
- [x] Update composition routes to target explicit input vs signal kinds
- [x] Update codegen rendering for separate `INPUTS` and `SIGNALS`
- [x] Update generated-kernel runtime helpers to validate signal-triggered transitions formally
- [x] Update schema validation to accept signal route targets
- [x] Update RMAT-facing composition validation plumbing to accept signal route targets
- [x] Replace subset parity tests with exact set-equality tests
- [x] Add explicit signal coverage tests
- [x] Regenerate canonical specs/artifacts

### Validation
- [x] `cargo test -p meerkat-machine-schema --test schema_contracts --quiet`
- [x] `cargo test -p meerkat-machine-codegen --quiet`
- [x] `cargo test -p meerkat-machine-kernels --lib --quiet`
- [x] `make machine-check-drift`
- [x] `make verify-schema-freshness`
- [x] `make rmat-audit`
- [x] `make machine-verify`

### Closeout
- [x] `MeerkatMachine.inputs == runtime manifest`
- [x] `MobMachine.inputs == runtime manifest - exclusions`
- [x] every input has transition coverage
- [x] every signal has transition coverage
- [x] no undeclared route targets

## Plan vs Reality

### What matched the plan
- Canonical `inputs` are now driven from derived runtime command manifests instead of hand-maintained lists.
- Canonical machine schemas now distinguish runtime `inputs` from formal-only `signals`.
- Composition routes now model mixed target kinds explicitly.
- The identity-first formal surfaces are regenerated from the normalized schemas instead of being treated as a post-hoc TLC rerun.

### What differed from the plan
- Signals remain formal-only, but the generated-kernel test harness needed a signal-aware transition helper so formal validation could keep exercising signal-triggered transitions. This did not add a product runtime signal dispatch path.
- The Meerkat canonical runtime manifest needed to union six enums, not five, because ingress commands are real runtime commands and must remain in the canonical alphabet.
- The semantic-model renderer needed extra normalization work after the input/signal split because it had several hidden `inputs`-only assumptions in type-domain collection and transition binding typing.
