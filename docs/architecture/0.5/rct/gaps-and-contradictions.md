# Gaps And Contradictions

This file records the package-level contradictions that existed when the
docs-local RCT package was first derived, and how they were resolved.

Current state:

- there are no intentionally open package-level contradictions remaining in
  the `0.5` docs package
- the items below are kept as a closure ledger, not as an open decision list

## Resolved Package-Level Contradictions

| Former ID | Resolved issue | Final package stance | Authoritative home |
| --- | --- | --- | --- |
| `GAP-001` | Execution/workstream numbering differed between the execution plan and implementation plan | `plan.yaml` is the authoritative RCT phase model and carries the crosswalk to both documents | `rct/plan.yaml` |
| `GAP-002` | Runtime ingress taxonomy was not closed (`Projected`, continuation, `OperationInput`, contributor model) | `Projected` and top-level `SystemGenerated` do not survive; `ContinuationInput` and `OperationInput` are explicit; executed units may be runtime-authoritative multi-contributor staged runs | `meerkat_0_5_architecture_outline.md`, `meerkat_0_5_implementation_plan.md`, `rct/spec.yaml` |
| `GAP-003` | Runtime policy algebra was split between conceptual and current implementation terms | `0.5` exposes explicit `InterruptPolicy`, `DrainPolicy`, `RoutingDisposition`, queue discipline, and consume point while preserving the current runtime policy table as the behavioral baseline until machine authority changes | `meerkat_0_5_architecture_outline.md`, `meerkat_sm_nomenclature.md`, `rct/spec.yaml` |
| `GAP-004` | Canonical machine file layout differed across docs | Rust-native machine and composition authority lives under `meerkat-machine-schema/src/catalog/`, and generated outputs live under `specs/machines/<machine>/` and `specs/compositions/<bundle>/` plus owning-crate generated-kernel paths | `meerkat_machine_schema_workflow_spec.md`, `meerkat_0_5_implementation_plan.md` |
| `GAP-005` | JSON-RPC `event/push` end-state was under-decided | `session/external_event` is the canonical high-level RPC convenience method, `runtime/accept` remains authoritative, and `event/push` is deleted after at most one early-adapter phase | `meerkat_surface_cutover_matrix.md`, `meerkat_0_5_implementation_plan.md`, `rct/spec.yaml` |
| `GAP-006` | REST external-event route naming/versioning was open | `POST /sessions/{id}/external-events` is the canonical high-level REST convenience route, while `POST /runtime/{id}/accept` remains authoritative | `meerkat_surface_cutover_matrix.md`, `meerkat_0_5_implementation_plan.md`, `rct/spec.yaml` |
| `GAP-007` | WASM bootstrap semantics were contradictory | runtime bootstrap is mandatory, browser execution stays runtime-backed, browser-local tools are runtime-scoped, and unsupported capabilities fail early | `meerkat_0_5_architecture_outline.md`, `meerkat_surface_cutover_matrix.md`, `rct/spec.yaml` |
| `GAP-008` | `SubAgentManager` final shape was not locked | there is no surviving subagent mechanism in `0.5`; subagents are mobs, `SubAgentManager` is deleted, and child-agent flows use the mob control plane | `meerkat_ops_lifecycle_seam_spec.md`, `meerkat_0_5_implementation_plan.md`, `rct/spec.yaml` |
| `GAP-009` | External-tool outward contract was split between existing notice shapes | one canonical typed outward lifecycle delta/notice contract is authoritative; transport/text notices are derivative projections only | `meerkat_surface_cutover_matrix.md`, `meerkat_0_5_implementation_plan.md`, `rct/spec.yaml` |
| `GAP-010` | Advanced machine-authority harness ordering was ambiguous in the critical path | machine-authority harness scaffolding lands before richer generated-kernel convergence and any surface dependencies on those kernels | `rct/plan.yaml`, `rct/checklist.yaml` |

## Final Closure Notes

The remaining work in this package is implementation work, not architecture
decision work.

The anti-drift rule is:

1. docs and RCT package coordinate the migration
2. landed Rust-native catalog definitions and generated kernels become the
   long-term semantic source of truth
3. CI must reject any drift between machine authority and downstream
   code/docs/verification artifacts
