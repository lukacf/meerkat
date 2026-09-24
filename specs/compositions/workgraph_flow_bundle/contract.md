# workgraph_flow_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `work_execution`: `WorkExecutionLifecycleMachine` @ actor `work_execution_authority`

## Routes

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `(none)`

## Scheduler Rules
- `(none)`

## Structural Requirements
- `flow_launch_handoff` — WorkExecution effect FlowLaunchRequested is realized only through its typed owner handoff
- `flow_observation_handoff` — WorkExecution effect FlowLaunchAccepted is realized only through its typed owner handoff
- `uncertain_launch_handoff` — WorkExecution effect FlowLaunchUncertain is realized only through its typed owner handoff
- `quarantined_launch_handoff` — WorkExecution effect FlowLaunchQuarantined is realized only through its typed owner handoff
- `success_evidence_handoff` — WorkExecution effect EvidenceProjectionRequested is realized only through its typed owner handoff
- `failure_evidence_handoff` — WorkExecution effect FlowFailureEvidenceProjectionRequested is realized only through its typed owner handoff
- `cancellation_evidence_handoff` — WorkExecution effect FlowCancellationEvidenceProjectionRequested is realized only through its typed owner handoff
- `launch_failure_evidence_handoff` — WorkExecution effect LaunchFailureEvidenceProjectionRequested is realized only through its typed owner handoff
- `work_closure_handoff` — WorkExecution effect WorkClosureRequested is realized only through its typed owner handoff

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `workgraph_flow_bridge_owner` (machine `WorkExecutionLifecycleMachine`): `crates/meerkat-mob/src/workgraph_flow.rs` — mechanical Mob composition facade realizes generated WorkExecution launch, evidence, and closure obligations and returns typed feedback
- `workgraph_flow_bundle_schema` (machine `WorkExecutionLifecycleMachine`): `crates/meerkat-machine-schema/src/catalog/compositions.rs` — formal WorkExecution lifecycle handoff composition for Mob Flow attempts

### Scenarios
- `flow-launch-feedback` — a durable WorkExecution launch obligation is realized by Mob and closed only by typed started, observed, uncertain, or failed feedback
- `flow-terminal-evidence-feedback` — successful, failed, and canceled Flow outcomes project idempotent evidence before their corresponding typed feedback advances WorkExecution
- `work-closure-feedback` — successful evidence requests WorkGraph closure and records either typed closure confirmation or policy refusal
