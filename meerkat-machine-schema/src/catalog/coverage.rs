use crate::{CompositionSchema, MachineSchema, SchedulerRule};

use super::{
    comms_drain_lifecycle::comms_drain_lifecycle_machine,
    compositions::{
        comms_drain_lifecycle_composition, continuation_runtime_bundle_composition,
        external_tool_bundle_composition, mob_bundle_composition, ops_peer_bundle_composition,
        ops_runtime_bundle_composition, peer_runtime_bundle_composition,
        runtime_pipeline_composition, surface_event_runtime_bundle_composition,
    },
    external_tool_surface::external_tool_surface_machine,
    flow_run::flow_run_machine,
    input_lifecycle::input_lifecycle_machine,
    mob_helper_result_anchor::mob_helper_result_anchor_machine,
    mob_lifecycle::mob_lifecycle_machine,
    mob_member_lifecycle_anchor::mob_member_lifecycle_anchor_machine,
    mob_orchestrator::mob_orchestrator_machine,
    mob_runtime_bridge_anchor::mob_runtime_bridge_anchor_machine,
    mob_wiring_anchor::mob_wiring_anchor_machine,
    ops_lifecycle::ops_lifecycle_machine,
    peer_comms::peer_comms_machine,
    peer_directory_reachability::peer_directory_reachability_machine,
    runtime_control::runtime_control_machine,
    runtime_ingress::runtime_ingress_machine,
    turn_execution::turn_execution_machine,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAnchor {
    pub id: String,
    pub path: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioCoverage {
    pub id: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCoverageEntry {
    pub name: String,
    pub anchor_ids: Vec<String>,
    pub scenario_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCoverageManifest {
    pub machine: String,
    pub code_anchors: Vec<CodeAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub transition_coverage: Vec<SemanticCoverageEntry>,
    pub effect_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionCoverageManifest {
    pub composition: String,
    pub code_anchors: Vec<CodeAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub route_coverage: Vec<SemanticCoverageEntry>,
    pub scheduler_rule_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

pub fn canonical_machine_coverage_manifests() -> Vec<MachineCoverageManifest> {
    vec![
        machine_manifest_from_schema(
            &input_lifecycle_machine(),
            &[
                anchor(
                    "input_state",
                    "meerkat-runtime/src/input_state.rs",
                    "authoritative input lifecycle record shape",
                ),
                anchor(
                    "input_ledger",
                    "meerkat-runtime/src/input_ledger.rs",
                    "runtime-owned lifecycle ledger precursor",
                ),
            ],
            &[
                scenario(
                    "queue-stage-apply-consume",
                    "accepted input queues, stages, applies, and is consumed at a boundary",
                ),
                scenario(
                    "supersede-coalesce",
                    "queued input is terminalized by supersession or coalescing",
                ),
                scenario(
                    "abandon",
                    "input is abandoned cleanly during reset/destroy style terminalization",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &runtime_control_machine(),
            &[
                anchor(
                    "runtime_state",
                    "meerkat-runtime/src/runtime_state.rs",
                    "runtime lifecycle state precursor",
                ),
                anchor(
                    "runtime_control_authority",
                    "meerkat-runtime/src/runtime_control_authority.rs",
                    "canonical runtime control authority and transition reducer",
                ),
                anchor(
                    "runtime_loop",
                    "meerkat-runtime/src/runtime_loop.rs",
                    "control-plane select loop and run coordination precursor",
                ),
                anchor(
                    "runtime_control_plane",
                    "meerkat-runtime/src/control_plane.rs",
                    "stop/preemption seam and completion-resolution precursor",
                ),
                anchor(
                    "runtime_session_adapter",
                    "meerkat-runtime/src/session_adapter.rs",
                    "surface-facing lifecycle and completion owner precursor",
                ),
            ],
            &[
                scenario(
                    "control-preempts-ingress",
                    "control commands preempt ordinary ingress work",
                ),
                scenario(
                    "prompt-queue",
                    "queued ordinary work waits for the next outer-loop turn without modifying the current run",
                ),
                scenario(
                    "prompt-steer",
                    "steered ordinary work drains into the active run at the earliest admissible boundary",
                ),
                scenario(
                    "begin-run-complete",
                    "runtime transitions idle to running to idle for a completed run",
                ),
                scenario(
                    "retire-stop-destroy",
                    "runtime transitions through retire/stop/destroy commands without reopening ordinary work",
                ),
                scenario(
                    "reset-terminates-waiters",
                    "reset abandons pending work and resolves completion waiters exactly once",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &runtime_ingress_machine(),
            &[
                anchor(
                    "runtime_input_taxonomy",
                    "meerkat-runtime/src/input.rs",
                    "runtime ingress input taxonomy precursor",
                ),
                anchor(
                    "runtime_queue",
                    "meerkat-runtime/src/queue.rs",
                    "ordered queue discipline precursor",
                ),
                anchor(
                    "runtime_ephemeral_driver",
                    "meerkat-runtime/src/driver/ephemeral.rs",
                    "ephemeral ingress mutation precursor",
                ),
                anchor(
                    "runtime_persistent_driver",
                    "meerkat-runtime/src/driver/persistent.rs",
                    "persistent ingress/recovery precursor",
                ),
                anchor(
                    "runtime_loop",
                    "meerkat-runtime/src/runtime_loop.rs",
                    "same-boundary contributor batching and staged run precursor",
                ),
            ],
            &[
                scenario(
                    "admit-and-stage-prefix",
                    "individually admitted inputs form a runtime-authored staged prefix",
                ),
                scenario(
                    "prompt-queue",
                    "queued user prompt enters ingress without immediate processing when already running",
                ),
                scenario(
                    "prompt-steer",
                    "steering user prompt enters ingress with immediate-processing intent",
                ),
                scenario(
                    "rollback-on-failure",
                    "failed or cancelled run restores staged contributors to the queue front",
                ),
                scenario(
                    "recover-retire-reset-destroy",
                    "recovery and lifecycle terminalization preserve contributor legality",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &ops_lifecycle_machine(),
            &[
                anchor(
                    "ops_vocab",
                    "meerkat-core/src/ops.rs",
                    "shared async-operation vocabulary precursor",
                ),
                anchor(
                    "mob_provisioner",
                    "meerkat-mob/src/runtime/provisioner.rs",
                    "mob-backed child lifecycle precursor",
                ),
                anchor(
                    "shell_job_manager",
                    "meerkat-tools/src/builtin/shell/job_manager.rs",
                    "background tool-operation lifecycle precursor",
                ),
            ],
            &[
                scenario(
                    "register-progress-terminal",
                    "async operation registers, reports progress, and reaches a terminal outcome",
                ),
                scenario(
                    "peer-ready-handoff",
                    "child operation hands off to peer comms at peer_ready",
                ),
                scenario(
                    "cancel-and-watch",
                    "async operation cancellation resolves watcher semantics exactly once",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &peer_comms_machine(),
            &[
                anchor(
                    "peer_classify",
                    "meerkat-comms/src/classify.rs",
                    "peer classification precursor",
                ),
                anchor(
                    "peer_inbox",
                    "meerkat-comms/src/inbox.rs",
                    "peer inbox and request/reservation registry precursor",
                ),
                anchor(
                    "peer_runtime",
                    "meerkat-comms/src/runtime/comms_runtime.rs",
                    "runtime comms owner precursor",
                ),
            ],
            &[
                scenario(
                    "trust-normalize-submit",
                    "trusted peer envelope is normalized and submitted exactly once",
                ),
                scenario(
                    "untrusted-drop",
                    "untrusted or invalid peer work is dropped before runtime admission",
                ),
                scenario(
                    "request-response-correlation",
                    "reservation/request state remains consistent across peer traffic",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &peer_directory_reachability_machine(),
            &[
                anchor(
                    "peer_directory_reachability_authority",
                    "meerkat-comms/src/peer_directory_reachability_authority.rs",
                    "canonical transient reachability authority and reconcile/send-result reducer",
                ),
                anchor(
                    "comms_runtime_directory_projection",
                    "meerkat-comms/src/runtime/comms_runtime.rs",
                    "runtime-owned resolved peer directory projection and send-result integration",
                ),
            ],
            &[
                scenario(
                    "reconcile-directory",
                    "resolved peer directory snapshot is reconciled into transient reachability state",
                ),
                scenario(
                    "send-success-marks-reachable",
                    "successful delivery marks an already resolved peer reachable",
                ),
                scenario(
                    "send-failure-marks-unreachable",
                    "offline or transport failures mark a resolved peer unreachable without inventing unknown peers",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &external_tool_surface_machine(),
            &[
                anchor(
                    "mcp_router",
                    "meerkat-mcp/src/router.rs",
                    "staged MCP surface lifecycle precursor",
                ),
                anchor(
                    "mcp_adapter",
                    "meerkat-mcp/src/adapter.rs",
                    "runtime-facing tool-surface adapter precursor",
                ),
                anchor(
                    "agent_tool_state",
                    "meerkat-core/src/agent/state.rs",
                    "turn-boundary external-tool update consumer precursor",
                ),
            ],
            &[
                scenario(
                    "add-reload-remove",
                    "surface add, reload, and removal produce canonical typed deltas",
                ),
                scenario(
                    "draining-removal",
                    "removing surfaces drain inflight work before final removal",
                ),
                scenario(
                    "runtime-scoped-browser-tools",
                    "browser-local tools remain runtime-scoped external surfaces",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &turn_execution_machine(),
            &[
                anchor(
                    "turn_state",
                    "meerkat-core/src/agent/state.rs",
                    "core turn loop state precursor",
                ),
                anchor(
                    "turn_runner",
                    "meerkat-core/src/agent/runner.rs",
                    "turn runner precursor",
                ),
                anchor(
                    "run_primitive",
                    "meerkat-core/src/lifecycle/run_primitive.rs",
                    "canonical run primitive input precursor",
                ),
                anchor(
                    "run_event",
                    "meerkat-core/src/lifecycle/run_event.rs",
                    "canonical run event/effect precursor",
                ),
            ],
            &[
                scenario(
                    "conversation-run",
                    "conversation run starts, applies boundaries, and completes cleanly",
                ),
                scenario(
                    "tool-and-retry-loop",
                    "tool calls and retry/yield semantics stay inside the turn owner",
                ),
                scenario(
                    "cancel-and-fail",
                    "cancelled and failed runs produce explicit terminal outcomes",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &mob_lifecycle_machine(),
            &[
                anchor(
                    "mob_lifecycle_state",
                    "meerkat-mob/src/runtime/state.rs",
                    "mob lifecycle state precursor",
                ),
                anchor(
                    "mob_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "serialized lifecycle owner precursor",
                ),
                anchor(
                    "mob_handle",
                    "meerkat-mob/src/runtime/handle.rs",
                    "public lifecycle handle precursor",
                ),
            ],
            &[
                scenario(
                    "start-stop-resume",
                    "mob lifecycle transitions through start/stop/resume cleanly",
                ),
                scenario(
                    "run-count-and-cleanup",
                    "active run count and cleanup semantics stay consistent",
                ),
                scenario(
                    "complete-destroy",
                    "completed/destroyed lifecycle phases stay terminal",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &mob_member_lifecycle_anchor_machine(),
            &[
                anchor(
                    "mob_runtime_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "mob actor observes child operation peer exposure and terminalization routes",
                ),
                anchor(
                    "mob_ops_adapter",
                    "meerkat-mob/src/runtime/ops_adapter.rs",
                    "runtime/ops bridge that carries operation lifecycle signals",
                ),
                anchor(
                    "mob_provisioner",
                    "meerkat-mob/src/runtime/provisioner.rs",
                    "member provisioning/retirement bridge where child operation lineage is surfaced",
                ),
            ],
            &[
                scenario(
                    "member-peer-exposure-observed",
                    "operation peer-ready exposure is mirrored into member lifecycle observation state",
                ),
                scenario(
                    "member-terminalization-observed",
                    "operation terminalization is mirrored into member lifecycle observation state",
                ),
                scenario(
                    "member-lifecycle-observation-lineage",
                    "member lifecycle anchor tracks observation lineage counters and sets without owning canonical lifecycle truth",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &mob_runtime_bridge_anchor_machine(),
            &[
                anchor(
                    "mob_runtime_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "mob actor emits runtime run submission/terminalization and stop-request boundary events",
                ),
                anchor(
                    "mob_provisioner",
                    "meerkat-mob/src/runtime/provisioner.rs",
                    "runtime session bridge and queue handling precursor for runtime bridge boundaries",
                ),
                anchor(
                    "mob_ops_adapter",
                    "meerkat-mob/src/runtime/ops_adapter.rs",
                    "ops/runtime linkage precursor for run-level bridge observations",
                ),
            ],
            &[
                scenario(
                    "runtime-run-submission-observed",
                    "runtime run submissions are mirrored into runtime-bridge observation state",
                ),
                scenario(
                    "runtime-run-terminal-observed",
                    "runtime run completion/failure/cancellation is mirrored into runtime-bridge observation state",
                ),
                scenario(
                    "runtime-stop-request-observed",
                    "runtime stop requests are mirrored into runtime-bridge observation state",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &mob_wiring_anchor_machine(),
            &[
                anchor(
                    "mob_runtime_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "peer wiring and admission routes mirrored into wiring observation state",
                ),
                anchor(
                    "mob_roster_authority",
                    "meerkat-mob/src/runtime/roster_authority.rs",
                    "roster/peer graph mutation precursor for wiring boundary observations",
                ),
                anchor(
                    "mob_edge_locks",
                    "meerkat-mob/src/runtime/edge_locks.rs",
                    "wire/unwire lock discipline precursor tied to observed wiring boundaries",
                ),
            ],
            &[
                scenario(
                    "operation-peer-trust-observed",
                    "operation peer-trust events are mirrored into wiring observation state",
                ),
                scenario(
                    "peer-input-admission-observed",
                    "peer input candidate admission is mirrored into wiring observation state",
                ),
                scenario(
                    "runtime-work-admission-observed",
                    "runtime work admission is mirrored into wiring observation state",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &mob_helper_result_anchor_machine(),
            &[
                anchor(
                    "mob_runtime_handle",
                    "meerkat-mob/src/runtime/handle.rs",
                    "helper-facing surfaces that return run/member result classes",
                ),
                anchor(
                    "mob_runtime_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "runtime command handling path that mirrors terminal classes for helper surfaces",
                ),
                anchor(
                    "mob_terminalization",
                    "meerkat-mob/src/runtime/terminalization.rs",
                    "run terminalization precursor for helper-result class observations",
                ),
            ],
            &[
                scenario(
                    "helper-completed-class-observed",
                    "completed run classes are mirrored into helper-result observation state",
                ),
                scenario(
                    "helper-failed-class-observed",
                    "failed and cancelled run classes are mirrored into helper-result observation state",
                ),
                scenario(
                    "helper-force-cancel-observed",
                    "force-cancel helper classification signals are mirrored into helper-result observation state",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &mob_member_lifecycle_anchor_machine(),
            &[
                anchor(
                    "mob_member_lifecycle_anchor_schema",
                    "meerkat-machine-schema/src/catalog/mob_member_lifecycle_anchor.rs",
                    "formal observation anchor for member lifecycle boundary routes",
                ),
                anchor(
                    "mob_ops_adapter",
                    "meerkat-mob/src/runtime/ops_adapter.rs",
                    "member operation lifecycle observation and peer-ready reporting precursor",
                ),
                anchor(
                    "mob_handle",
                    "meerkat-mob/src/runtime/handle.rs",
                    "helper/member-facing lifecycle observation precursor",
                ),
            ],
            &[scenario(
                "member-peer-exposure-terminalization",
                "member lifecycle anchor observes peer exposure and terminalization routes",
            )],
        ),
        machine_manifest_from_schema(
            &mob_runtime_bridge_anchor_machine(),
            &[
                anchor(
                    "mob_runtime_bridge_anchor_schema",
                    "meerkat-machine-schema/src/catalog/mob_runtime_bridge_anchor.rs",
                    "formal observation anchor for runtime bridge routes",
                ),
                anchor(
                    "mob_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "runtime-bridge command handling precursor",
                ),
                anchor(
                    "mob_provisioner",
                    "meerkat-mob/src/runtime/provisioner.rs",
                    "runtime-backed member provisioning precursor",
                ),
            ],
            &[scenario(
                "runtime-run-bridge-observation",
                "runtime bridge anchor observes submitted, terminal, and stop-request run routes",
            )],
        ),
        machine_manifest_from_schema(
            &mob_wiring_anchor_machine(),
            &[
                anchor(
                    "mob_wiring_anchor_schema",
                    "meerkat-machine-schema/src/catalog/mob_wiring_anchor.rs",
                    "formal observation anchor for mob wiring routes",
                ),
                anchor(
                    "mob_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "peer admission and wiring mutation precursor",
                ),
                anchor(
                    "mob_ops_adapter",
                    "meerkat-mob/src/runtime/ops_adapter.rs",
                    "runtime work and trusted-peer observation precursor",
                ),
            ],
            &[scenario(
                "peer-input-runtime-work-observation",
                "wiring anchor observes peer-input admission, runtime-work admission, and trusted operation peers",
            )],
        ),
        machine_manifest_from_schema(
            &mob_helper_result_anchor_machine(),
            &[
                anchor(
                    "mob_helper_result_anchor_schema",
                    "meerkat-machine-schema/src/catalog/mob_helper_result_anchor.rs",
                    "formal observation anchor for helper-facing terminal result classes",
                ),
                anchor(
                    "mob_handle",
                    "meerkat-mob/src/runtime/handle.rs",
                    "helper result classification precursor",
                ),
                anchor(
                    "mob_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "runtime terminal outcome observation precursor",
                ),
            ],
            &[scenario(
                "helper-result-observation",
                "helper result anchor observes completed, failed, cancelled, and force-cancelled helper outcomes",
            )],
        ),
        machine_manifest_from_schema(
            &flow_run_machine(),
            &[
                anchor(
                    "flow_run_aggregate",
                    "meerkat-mob/src/run.rs",
                    "durable flow run aggregate precursor",
                ),
                anchor(
                    "flow_runtime",
                    "meerkat-mob/src/runtime/flow.rs",
                    "flow dispatch precursor",
                ),
                anchor(
                    "flow_terminalization",
                    "meerkat-mob/src/runtime/terminalization.rs",
                    "CAS-guarded terminalization precursor",
                ),
            ],
            &[
                scenario(
                    "create-dispatch-complete",
                    "flow run creates, dispatches steps, and records completion",
                ),
                scenario(
                    "dependency-ready-evaluation",
                    "dependency state drives ready-set and next-step admission",
                ),
                scenario(
                    "terminalize-on-failure-or-cancel",
                    "failed or canceled runs terminalize deterministically",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &mob_orchestrator_machine(),
            &[
                anchor(
                    "mob_runtime_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "orchestration owner precursor",
                ),
                anchor(
                    "mob_runtime_builder",
                    "meerkat-mob/src/runtime/builder.rs",
                    "runtime-mode/builder orchestration precursor",
                ),
                anchor(
                    "mob_definition",
                    "meerkat-mob/src/definition.rs",
                    "definition-level coordinator/topology precursor",
                ),
            ],
            &[
                scenario(
                    "coordinator-bind-and-supervise",
                    "orchestrator binds coordinator authority and supervision",
                ),
                scenario(
                    "pending-spawn-ledger",
                    "pending spawn and completion semantics remain explicit",
                ),
                scenario(
                    "topology-revision",
                    "topology/orchestration revisions remain monotonic and owned",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &comms_drain_lifecycle_machine(),
            &[
                anchor(
                    "comms_drain_authority",
                    "meerkat-runtime/src/comms_drain_lifecycle_authority.rs",
                    "comms drain lifecycle authority (sealed mutator + evaluate)",
                ),
                anchor(
                    "session_adapter_drain",
                    "meerkat-runtime/src/session_adapter.rs",
                    "session adapter comms drain slot wiring and effect execution",
                ),
                anchor(
                    "comms_drain_spawn",
                    "meerkat-runtime/src/comms_drain.rs",
                    "comms drain task spawn and loop implementation",
                ),
            ],
            &[
                scenario(
                    "spawn-run-exit",
                    "drain task spawns, runs, and exits cleanly with suppression lifecycle",
                ),
                scenario(
                    "persistent-respawn",
                    "persistent-host drain respawns after transient failure",
                ),
                scenario(
                    "stop-abort",
                    "drain task is stopped or aborted and suppression is lifted",
                ),
            ],
        ),
    ]
}

pub fn canonical_composition_coverage_manifests() -> Vec<CompositionCoverageManifest> {
    vec![
        composition_manifest_from_schema(
            &runtime_pipeline_composition(),
            &[
                anchor(
                    "runtime_loop",
                    "meerkat-runtime/src/runtime_loop.rs",
                    "runtime orchestration precursor for control/ingress/execution",
                ),
                anchor(
                    "core_executor",
                    "meerkat-core/src/lifecycle/core_executor.rs",
                    "turn execution bridge precursor",
                ),
                anchor(
                    "run_event",
                    "meerkat-core/src/lifecycle/run_event.rs",
                    "boundary/completion effect surface precursor",
                ),
            ],
            &[
                scenario(
                    "prompt-queue",
                    "queued prompt stays ordinary and waits for the next normal boundary when a run is already active",
                ),
                scenario(
                    "prompt-steer",
                    "steering prompt requests ASAP admission-to-ingress handling while preserving ordinary-work semantics",
                ),
                scenario(
                    "runtime-success-path",
                    "staged work begins a run, applies a boundary, and completes",
                ),
                scenario(
                    "runtime-failure-rollback",
                    "failed run rolls staged contributors back before steady state",
                ),
                scenario(
                    "runtime-cancel-rollback",
                    "cancelled run rolls staged contributors back before steady state",
                ),
                scenario(
                    "control-preemption",
                    "control-plane work preempts ordinary ingress scheduling",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &surface_event_runtime_bundle_composition(),
            &[
                anchor(
                    "cli_stdin_events",
                    "meerkat-cli/src/stdin_events.rs",
                    "CLI external-event ingestion precursor",
                ),
                anchor(
                    "rest_event_surface",
                    "meerkat-rest/src/lib.rs",
                    "REST external-event surface precursor",
                ),
                anchor(
                    "rpc_event_surface",
                    "meerkat-rpc/src/handlers/event.rs",
                    "JSON-RPC external-event surface precursor",
                ),
                anchor(
                    "wasm_runtime_surface",
                    "meerkat-web-runtime/src/lib.rs",
                    "WASM/browser external-event surface precursor",
                ),
                anchor(
                    "surface_cutover_matrix",
                    "docs/architecture/0.5/meerkat_surface_cutover_matrix.md",
                    "canonical external-event surface contract",
                ),
            ],
            &[
                scenario(
                    "cli-surface-event-admission",
                    "CLI stdin and host-driven external events use canonical runtime admission",
                ),
                scenario(
                    "rest-surface-event-admission",
                    "REST external events use canonical runtime admission",
                ),
                scenario(
                    "rpc-surface-event-admission",
                    "JSON-RPC external events use canonical runtime admission",
                ),
                scenario(
                    "wasm-surface-event-admission",
                    "browser/WASM external events use canonical runtime admission",
                ),
                scenario(
                    "surface-event-failure",
                    "surface-originated runs still route failure through ingress and control",
                ),
                scenario(
                    "surface-control-preemption",
                    "control-plane work still preempts surface-originated ingress",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &continuation_runtime_bundle_composition(),
            &[
                anchor(
                    "host_mode_cutover",
                    "docs/architecture/0.5/meerkat_host_mode_cutover_spec.md",
                    "runtime-owned continuation scheduling contract",
                ),
                anchor(
                    "runtime_comms_drain",
                    "meerkat-runtime/src/comms_drain.rs",
                    "comms inbox drain feeding typed inputs into the runtime adapter",
                ),
                anchor(
                    "agent_comms_impl",
                    "meerkat-core/src/agent/comms_impl.rs",
                    "terminal peer response continuation precursor",
                ),
                anchor(
                    "agent_runner",
                    "meerkat-core/src/agent/runner.rs",
                    "continuation acceptance precursor",
                ),
            ],
            &[
                scenario(
                    "terminal-response-continuation",
                    "terminal peer responses schedule continuation through runtime-owned admission",
                ),
                scenario(
                    "host-mode-continuation",
                    "host-mode continuation still runs through the canonical runtime path",
                ),
                scenario(
                    "continuation-control-preemption",
                    "control-plane work preempts continuation ingress scheduling",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &peer_runtime_bundle_composition(),
            &[
                anchor(
                    "peer_classify",
                    "meerkat-comms/src/classify.rs",
                    "peer normalization precursor",
                ),
                anchor(
                    "peer_runtime",
                    "meerkat-comms/src/runtime/comms_runtime.rs",
                    "runtime-facing peer delivery precursor",
                ),
                anchor(
                    "runtime_loop",
                    "meerkat-runtime/src/runtime_loop.rs",
                    "runtime admission/control precursor",
                ),
            ],
            &[
                scenario(
                    "peer-message-admission",
                    "peer-classified work reaches runtime only through canonical admission",
                ),
                scenario(
                    "trust-before-admission",
                    "trust/classification is fixed before runtime sees peer work",
                ),
                scenario(
                    "no-direct-host-bypass",
                    "peer work does not bypass the runtime path",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &ops_runtime_bundle_composition(),
            &[
                anchor(
                    "ops_vocab",
                    "meerkat-core/src/ops.rs",
                    "shared async-operation vocabulary precursor",
                ),
                anchor(
                    "runtime_loop",
                    "meerkat-runtime/src/runtime_loop.rs",
                    "runtime operation admission precursor",
                ),
                anchor(
                    "shell_job_manager",
                    "meerkat-tools/src/builtin/shell/job_manager.rs",
                    "background async-op source precursor",
                ),
            ],
            &[
                scenario(
                    "operation-event-reentry",
                    "async-operation events re-enter runtime as operation input",
                ),
                scenario(
                    "ops-runtime-terminality",
                    "lifecycle and runtime both observe required terminal outcomes",
                ),
                scenario(
                    "ops-control-preemption",
                    "control-plane work still outranks ops-driven ingress",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &external_tool_bundle_composition(),
            &[
                anchor(
                    "mcp_router",
                    "meerkat-mcp/src/router.rs",
                    "tool-surface lifecycle precursor",
                ),
                anchor(
                    "agent_tool_state",
                    "meerkat-core/src/agent/state.rs",
                    "turn-boundary tool update consumer precursor",
                ),
                anchor(
                    "surface_projection",
                    "meerkat/src/surface.rs",
                    "surface projection precursor",
                ),
            ],
            &[
                scenario(
                    "tool-delta-to-runtime",
                    "external-tool deltas reach runtime through canonical control/runtime surfaces",
                ),
                scenario(
                    "reload-remove-during-turns",
                    "live tool surface changes coordinate with turn boundaries",
                ),
                scenario(
                    "browser-local-tool-surface",
                    "WASM/browser local tools follow the same runtime-owned tool surface",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &ops_peer_bundle_composition(),
            &[
                anchor(
                    "ops_lifecycle_shell",
                    "meerkat-runtime/src/ops_lifecycle.rs",
                    "ops lifecycle shell that handles ExposeOperationPeer effect",
                ),
                anchor(
                    "comms_runtime",
                    "meerkat-comms/src/runtime/comms_runtime.rs",
                    "add_trusted_peer wiring from ops to peer comms",
                ),
            ],
            &[scenario(
                "peer-ready-handoff",
                "ops-lifecycle PeerReady triggers peer-comms trust establishment",
            )],
        ),
        composition_manifest_from_schema(
            &mob_bundle_composition(),
            &[
                anchor(
                    "mob_runtime_actor",
                    "meerkat-mob/src/runtime/actor.rs",
                    "mob orchestration precursor",
                ),
                anchor(
                    "mob_member_handle",
                    "meerkat-mob/src/runtime/handle.rs",
                    "member-directed delivery capability",
                ),
                anchor(
                    "flow_runtime",
                    "meerkat-mob/src/runtime/flow.rs",
                    "flow dispatch precursor",
                ),
                anchor(
                    "peer_runtime",
                    "meerkat-comms/src/runtime/comms_runtime.rs",
                    "mob member peer communication precursor",
                ),
                anchor(
                    "wasm_example_031",
                    "examples/031-wasm-mini-diplomacy-sh/web/src/main.ts",
                    "mob-based WASM example coverage",
                ),
                anchor(
                    "wasm_example_032",
                    "examples/032-wasm-webcm-agent/web/src/main.ts",
                    "browser mob workflow coverage",
                ),
                anchor(
                    "wasm_example_033",
                    "examples/033-the-office-demo-sh/web/src/main.ts",
                    "browser local-tool + mob coverage",
                ),
            ],
            &[
                scenario(
                    "mob-flow-dispatch",
                    "flow step work enters runtime only through canonical admission",
                ),
                scenario(
                    "mob-child-report-back",
                    "mob-backed child work reports progress and terminality through ops lifecycle",
                ),
                scenario(
                    "mob-peer-orchestration",
                    "member-directed communication and orchestration stay inside the mob stack",
                ),
                scenario(
                    "wasm-mob-examples",
                    "browser mob examples continue to fit the canonical mob/comms/runtime model",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &comms_drain_lifecycle_composition(),
            &[
                anchor(
                    "comms_drain_authority",
                    "meerkat-core/src/comms_drain_lifecycle_authority.rs",
                    "comms drain lifecycle authority (sealed mutator + evaluate)",
                ),
                anchor(
                    "comms_drain_protocol",
                    "meerkat-core/src/generated/protocol_comms_drain_spawn.rs",
                    "generated spawn protocol helper for comms drain handoff",
                ),
                anchor(
                    "comms_drain_impl",
                    "meerkat-core/src/agent/comms_impl.rs",
                    "comms drain shell implementation (effect realization + feedback)",
                ),
            ],
            &[
                scenario(
                    "spawn-feedback-cycle",
                    "SpawnDrainTask obligation is closed by TaskSpawned or TaskExited feedback",
                ),
                scenario(
                    "abort-terminal-closure",
                    "AbortDrainTask obligation closes on terminal phase without explicit feedback",
                ),
            ],
        ),
    ]
}

fn machine_manifest_from_schema(
    schema: &MachineSchema,
    code_anchors: &[CodeAnchor],
    scenarios: &[ScenarioCoverage],
) -> MachineCoverageManifest {
    let anchor_ids = code_anchors
        .iter()
        .map(|anchor| anchor.id.clone())
        .collect::<Vec<_>>();
    let scenario_ids = scenarios
        .iter()
        .map(|scenario| scenario.id.clone())
        .collect::<Vec<_>>();

    MachineCoverageManifest {
        machine: schema.machine.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        transition_coverage: schema
            .transitions
            .iter()
            .map(|transition| {
                semantic_item(
                    &transition.name,
                    &anchor_ids,
                    &machine_scenario_ids(&schema.machine, &transition.name, &scenario_ids),
                )
            })
            .collect(),
        effect_coverage: schema
            .effects
            .variants
            .iter()
            .map(|variant| {
                semantic_item(
                    &variant.name,
                    &anchor_ids,
                    &machine_scenario_ids(&schema.machine, &variant.name, &scenario_ids),
                )
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| {
                semantic_item(
                    &invariant.name,
                    &anchor_ids,
                    &machine_scenario_ids(&schema.machine, &invariant.name, &scenario_ids),
                )
            })
            .collect(),
    }
}

fn composition_manifest_from_schema(
    schema: &CompositionSchema,
    code_anchors: &[CodeAnchor],
    scenarios: &[ScenarioCoverage],
) -> CompositionCoverageManifest {
    let anchor_ids = code_anchors
        .iter()
        .map(|anchor| anchor.id.clone())
        .collect::<Vec<_>>();
    let scenario_ids = scenarios
        .iter()
        .map(|scenario| scenario.id.clone())
        .collect::<Vec<_>>();

    CompositionCoverageManifest {
        composition: schema.name.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        route_coverage: schema
            .routes
            .iter()
            .map(|route| {
                semantic_item(
                    &route.name,
                    &anchor_ids,
                    &composition_scenario_ids(&schema.name, "route", &route.name, &scenario_ids),
                )
            })
            .collect(),
        scheduler_rule_coverage: schema
            .scheduler_rules
            .iter()
            .map(|rule| {
                let name = scheduler_rule_name(rule);
                semantic_item(
                    &name,
                    &anchor_ids,
                    &composition_scenario_ids(&schema.name, "scheduler", &name, &scenario_ids),
                )
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| {
                semantic_item(
                    &invariant.name,
                    &anchor_ids,
                    &composition_scenario_ids(
                        &schema.name,
                        "invariant",
                        &invariant.name,
                        &scenario_ids,
                    ),
                )
            })
            .collect(),
    }
}

fn semantic_item(
    name: &str,
    anchor_ids: &[String],
    scenario_ids: &[String],
) -> SemanticCoverageEntry {
    SemanticCoverageEntry {
        name: name.into(),
        anchor_ids: anchor_ids.to_vec(),
        scenario_ids: scenario_ids.to_vec(),
    }
}

fn machine_scenario_ids(machine: &str, item_name: &str, all_scenarios: &[String]) -> Vec<String> {
    let normalized = normalize_token(item_name);
    let mut hints = Vec::new();

    match machine {
        "InputLifecycleMachine" => {
            if normalized.contains("supersede") || normalized.contains("coalesce") {
                hints.extend(["supersede", "coalesce"]);
            } else if normalized.contains("abandon")
                || normalized.contains("destroy")
                || normalized.contains("reset")
                || normalized.contains("retire")
            {
                hints.extend(["abandon", "destroy", "reset", "retire"]);
            } else {
                hints.extend(["queue", "stage", "apply", "consume"]);
            }
        }
        "RuntimeControlMachine" => {
            if normalized.contains("retire")
                || normalized.contains("stop")
                || normalized.contains("destroy")
                || normalized.contains("recover")
                || normalized.contains("resume")
                || normalized.contains("reset")
            {
                hints.extend(["retire", "stop", "destroy", "recover", "resume", "reset"]);
            } else if normalized.contains("beginrun")
                || normalized.contains("run")
                || normalized.contains("submitrunprimitive")
            {
                hints.extend(["begin", "run", "complete"]);
            } else {
                hints.extend(["control", "admission", "ingress", "preempt"]);
            }
        }
        "RuntimeIngressMachine" => {
            if normalized.contains("runfailed") || normalized.contains("runcancelled") {
                hints.extend(["rollback", "failure", "cancel"]);
            } else if normalized.contains("recover")
                || normalized.contains("retire")
                || normalized.contains("reset")
                || normalized.contains("destroy")
            {
                hints.extend(["recover", "retire", "reset", "destroy"]);
            } else if normalized.contains("supersede") || normalized.contains("coalesce") {
                hints.extend(["coalesce", "supersede", "admit"]);
            } else {
                hints.extend(["admit", "stage", "prefix"]);
            }
        }
        "OpsLifecycleMachine" => {
            if normalized.contains("fail") || normalized.contains("cancel") {
                hints.extend(["fail", "cancel", "terminal"]);
            } else if normalized.contains("progress") || normalized.contains("peerready") {
                hints.extend(["progress", "peer"]);
            } else {
                hints.extend(["register", "provision", "running", "terminal"]);
            }
        }
        "PeerCommsMachine" => {
            if normalized.contains("trust") || normalized.contains("receive") {
                hints.extend(["trust", "peer", "receive"]);
            } else if normalized.contains("submit") || normalized.contains("candidate") {
                hints.extend(["submit", "peer", "queue"]);
            } else {
                hints.extend(["peer", "trust", "submit"]);
            }
        }
        "PeerDirectoryReachabilityMachine" => {
            if normalized.contains("reconcile") || normalized.contains("directory") {
                hints.extend(["reconcile", "directory"]);
            } else if normalized.contains("succeed") || normalized.contains("reachable") {
                hints.extend(["success", "reachable"]);
            } else {
                hints.extend(["failure", "unreachable", "send"]);
            }
        }
        "ExternalToolSurfaceMachine" => {
            if normalized.contains("reload") || normalized.contains("remove") {
                hints.extend(["reload", "remove", "tool"]);
            } else if normalized.contains("drain") || normalized.contains("fail") {
                hints.extend(["drain", "fail", "tool"]);
            } else {
                hints.extend(["tool", "surface", "stage", "apply", "browser"]);
            }
        }
        "TurnExecutionMachine" => {
            if normalized.contains("tool") {
                hints.extend(["tool", "loop"]);
            } else if normalized.contains("retry") || normalized.contains("recoverable") {
                hints.extend(["retry", "failure"]);
            } else if normalized.contains("fatal") || normalized.contains("fail") {
                hints.extend(["fatal", "failure"]);
            } else if normalized.contains("cancel") {
                hints.extend(["cancel"]);
            } else {
                hints.extend(["conversation", "boundary", "success"]);
            }
        }
        "MobLifecycleMachine" => {
            if normalized.contains("fail") || normalized.contains("cancel") {
                hints.extend(["fail", "cancel", "terminal"]);
            } else if normalized.contains("complete") || normalized.contains("retire") {
                hints.extend(["complete", "terminal"]);
            } else {
                hints.extend(["register", "running", "provision"]);
            }
        }
        "FlowRunMachine" => {
            if normalized.contains("fail") {
                hints.extend(["fail", "terminal"]);
            } else if normalized.contains("cancel") {
                hints.extend(["cancel", "terminal"]);
            } else if normalized.contains("skip") {
                hints.extend(["skip", "terminal"]);
            } else if normalized.contains("output") || normalized.contains("complete") {
                hints.extend(["output", "complete"]);
            } else {
                hints.extend(["create", "dispatch", "step", "run"]);
            }
        }
        "MobOrchestratorMachine" => {
            if normalized.contains("coordinator") {
                hints.extend(["coordinator", "supervise"]);
            } else if normalized.contains("spawn") {
                hints.extend(["spawn", "pending"]);
            } else if normalized.contains("flow") {
                hints.extend(["flow"]);
            } else {
                hints.extend(["topology", "coordinator", "flow"]);
            }
        }
        "CommsDrainLifecycleMachine" => {
            if normalized.contains("stop") || normalized.contains("abort") {
                hints.extend(["stop", "abort"]);
            } else if normalized.contains("exited") || normalized.contains("respawn") {
                hints.extend(["respawn", "persistent"]);
            } else {
                hints.extend(["spawn", "run", "exit"]);
            }
        }
        _ => {}
    }

    select_matching_scenarios(all_scenarios, &hints)
}

fn composition_scenario_ids(
    composition: &str,
    kind: &str,
    item_name: &str,
    all_scenarios: &[String],
) -> Vec<String> {
    let normalized = normalize_token(item_name);
    match composition {
        "runtime_pipeline" => {
            if normalized.contains("preempt") || kind == "scheduler" {
                select_exact_scenarios(all_scenarios, &["control-preemption"])
            } else if normalized.contains("cancel") {
                select_exact_scenarios(all_scenarios, &["runtime-cancel-rollback"])
            } else if normalized.contains("fail") {
                select_exact_scenarios(all_scenarios, &["runtime-failure-rollback"])
            } else {
                select_exact_scenarios(all_scenarios, &["runtime-success-path"])
            }
        }
        "surface_event_runtime_bundle" => {
            if normalized.contains("preempt") || kind == "scheduler" {
                select_exact_scenarios(all_scenarios, &["surface-control-preemption"])
            } else if normalized.contains("fail") {
                select_exact_scenarios(all_scenarios, &["surface-event-failure"])
            } else {
                select_exact_scenarios(
                    all_scenarios,
                    &[
                        "cli-surface-event-admission",
                        "rest-surface-event-admission",
                        "rpc-surface-event-admission",
                        "wasm-surface-event-admission",
                    ],
                )
            }
        }
        "continuation_runtime_bundle" => {
            if normalized.contains("preempt") || kind == "scheduler" {
                select_exact_scenarios(all_scenarios, &["continuation-control-preemption"])
            } else {
                select_exact_scenarios(
                    all_scenarios,
                    &["terminal-response-continuation", "host-mode-continuation"],
                )
            }
        }
        "peer_runtime_bundle" => {
            if normalized.contains("preempt") || kind == "scheduler" {
                select_exact_scenarios(all_scenarios, &["no-direct-host-bypass"])
            } else {
                select_exact_scenarios(
                    all_scenarios,
                    &["peer-message-admission", "trust-before-admission"],
                )
            }
        }
        "ops_peer_bundle" => select_exact_scenarios(all_scenarios, &["peer-ready-handoff"]),
        "ops_runtime_bundle" => {
            if normalized.contains("preempt") || kind == "scheduler" {
                select_exact_scenarios(all_scenarios, &["ops-control-preemption"])
            } else if normalized.contains("runcompleted")
                || normalized.contains("runfailed")
                || normalized.contains("runcancelled")
            {
                select_exact_scenarios(all_scenarios, &["ops-runtime-terminality"])
            } else {
                select_exact_scenarios(all_scenarios, &["operation-event-reentry"])
            }
        }
        "external_tool_bundle" => {
            if normalized.contains("preempt") || kind == "scheduler" {
                select_exact_scenarios(all_scenarios, &["tool-delta-to-runtime"])
            } else if normalized.contains("boundary") || normalized.contains("completion") {
                select_exact_scenarios(
                    all_scenarios,
                    &["reload-remove-during-turns", "browser-local-tool-surface"],
                )
            } else {
                select_exact_scenarios(
                    all_scenarios,
                    &["tool-delta-to-runtime", "browser-local-tool-surface"],
                )
            }
        }
        "mob_bundle" => {
            if normalized.contains("flow") || normalized.contains("step") {
                select_exact_scenarios(all_scenarios, &["mob-flow-dispatch", "wasm-mob-examples"])
            } else if normalized.contains("peer") {
                select_exact_scenarios(
                    all_scenarios,
                    &["mob-peer-orchestration", "wasm-mob-examples"],
                )
            } else if normalized.contains("op") || normalized.contains("async") {
                select_exact_scenarios(all_scenarios, &["mob-child-report-back"])
            } else if normalized.contains("preempt") || kind == "scheduler" {
                select_exact_scenarios(all_scenarios, &["mob-peer-orchestration"])
            } else {
                select_exact_scenarios(
                    all_scenarios,
                    &["mob-peer-orchestration", "wasm-mob-examples"],
                )
            }
        }
        "comms_drain_lifecycle" => {
            if normalized.contains("abort") {
                select_exact_scenarios(all_scenarios, &["abort-terminal-closure"])
            } else {
                select_exact_scenarios(all_scenarios, &["spawn-feedback-cycle"])
            }
        }
        _ => select_matching_scenarios(all_scenarios, &[]),
    }
}

fn select_matching_scenarios(all_scenarios: &[String], hints: &[&str]) -> Vec<String> {
    let normalized_scenarios = all_scenarios
        .iter()
        .map(|scenario| (scenario.clone(), normalize_token(scenario)))
        .collect::<Vec<_>>();

    let mut matched = normalized_scenarios
        .iter()
        .filter(|(_, scenario)| {
            hints
                .iter()
                .any(|hint| scenario.contains(&normalize_token(hint)))
        })
        .map(|(scenario, _)| scenario.clone())
        .collect::<Vec<_>>();

    if matched.is_empty() {
        matched = all_scenarios
            .first()
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
    }

    matched
}

fn select_exact_scenarios(all_scenarios: &[String], wanted: &[&str]) -> Vec<String> {
    let wanted = wanted
        .iter()
        .map(|item| (*item).to_owned())
        .collect::<Vec<_>>();
    let matched = all_scenarios
        .iter()
        .filter(|scenario| wanted.iter().any(|item| item == *scenario))
        .cloned()
        .collect::<Vec<_>>();

    if matched.is_empty() {
        all_scenarios.first().cloned().into_iter().collect()
    } else {
        matched
    }
}

fn normalize_token(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
                    .to_string()
                    .chars()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn scheduler_rule_name(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!("PreemptWhenReady({higher}, {lower})")
        }
    }
}

fn anchor(id: &str, path: &str, note: &str) -> CodeAnchor {
    CodeAnchor {
        id: id.into(),
        path: path.into(),
        note: note.into(),
    }
}

fn scenario(id: &str, summary: &str) -> ScenarioCoverage {
    ScenarioCoverage {
        id: id.into(),
        summary: summary.into(),
    }
}
