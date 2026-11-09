# Toolkit catalog

Canonical Meerkat DSL definitions for toolkit-owned application lifecycles.
The first owner is the per-request `ToolkitAcquisitionMachine`, compiled once
from `src/dsl/acquisition.rs` and consumed by `meerkat_toolkit::setup`.

The machine binds admitted request identity, exact agent selection, verified
owner-observation references, deadline and original-task continuation. It does
not choose connectors, run agents, establish role readiness, authorize service
access, store secrets or schedule retries. The production bridge must supply
trusted observations and commit generated state plus durable handoffs under a
storage fence before any effect realization.

`acquisition_schema()` exposes the compiled authority's schema.
`acquisition_model_artifacts()` renders its TLA model and bounded profiles.
`coverage::acquisition_coverage()` declares explicit schema-to-source/test
attribution. Rendering and coverage declarations do not establish execution;
recorded test and TLC runs are separate evidence.

State wire conversion is lossless and rejects unknown fields. Every restored
state must pass `ToolkitAcquisitionMachineAuthority::recover_from_state` even
in release builds. Deserialization alone is not validation.

Package checks use the repository wrapper and explicitly select the
[native candidate workspace](../../native-candidate/README.md). They require
its retained development prerequisites and do not certify a released native
integration:

```
./scripts/repo-cargo test --manifest-path native-candidate/Cargo.toml -p toolkit-catalog --locked
./scripts/repo-cargo test --manifest-path native-candidate/Cargo.toml -p toolkit-catalog --release --locked
./scripts/repo-cargo clippy --manifest-path native-candidate/Cargo.toml -p toolkit-catalog --all-targets --locked -- -D warnings
./scripts/repo-cargo fmt --manifest-path native-candidate/Cargo.toml -p toolkit-catalog -- --check
```

This crate alone does not complete shared preparation, real work launch,
current readiness consumption, durable continuation delivery or host adoption.
