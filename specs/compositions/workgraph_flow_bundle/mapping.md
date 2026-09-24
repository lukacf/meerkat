# workgraph_flow_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `workgraph_flow_bundle`

### Code Anchors
- `workgraph_flow_bridge_owner` (machine `WorkExecutionLifecycleMachine`): `crates/meerkat-mob/src/workgraph_flow.rs` — mechanical Mob composition facade realizes generated WorkExecution launch, evidence, and closure obligations and returns typed feedback
- `workgraph_flow_bundle_schema` (machine `WorkExecutionLifecycleMachine`): `crates/meerkat-machine-schema/src/catalog/compositions.rs` — formal WorkExecution lifecycle handoff composition for Mob Flow attempts

### Scenarios
- `flow-launch-feedback` — a durable WorkExecution launch obligation is realized by Mob and closed only by typed started, observed, uncertain, or failed feedback
- `flow-terminal-evidence-feedback` — successful, failed, and canceled Flow outcomes project idempotent evidence before their corresponding typed feedback advances WorkExecution
- `work-closure-feedback` — successful evidence requests WorkGraph closure and records either typed closure confirmation or policy refusal

### Routes
- `(none)`

### Scheduler Rules
- `(none)`

### Invariants
- `flow_launch_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `flow_observation_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `uncertain_launch_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `quarantined_launch_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `success_evidence_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `failure_evidence_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `cancellation_evidence_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `launch_failure_evidence_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `work_closure_handoff`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
