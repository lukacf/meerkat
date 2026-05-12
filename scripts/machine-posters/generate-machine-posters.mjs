#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");
const outputDir = path.join(
  repoRoot,
  "docs-internal",
  "machine-posters",
);

const THEME = {
  bg: "#0b0d0f",
  shell: "#0f1317",
  plate: "#12171b",
  panel: "#141a1f",
  card: "#171e24",
  border: "#2d353c",
  grid: "rgba(234, 226, 214, 0.035)",
  ink: "#f4efe5",
  muted: "#b6aea0",
  faint: "#7d7a74",
  input: "#84bfd7",
  signal: "#d8aa57",
  effect: "#8cbe95",
  routed: "#cf6f52",
  local: "#7ab2c7",
  external: "#b7cf7b",
  sink: "#2b1212",
};

const MACHINE_SPECS = [
  {
    id: "meerkat_machine",
    title: "MeerkatMachine",
    subtitle: "Lifecycle / Transition / Visibility / Effect Architecture",
    tlaPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "meerkat_machine",
      "model.tla",
    ),
    contractPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "meerkat_machine",
      "contract.md",
    ),
    catalogPath: path.join(
      repoRoot,
      "meerkat-machine-schema",
      "src",
      "catalog",
      "dsl",
      "meerkat_machine.rs",
    ),
    accent: "#c96d54",
    layout: {
      plate: { width: 1980, height: 1260 },
      phases: {
        Initializing: {
          x: 825,
          y: 54,
          w: 300,
          h: 84,
          subtitle: "bootstrap / zero state",
        },
        Idle: {
          x: 150,
          y: 280,
          w: 280,
          h: 94,
          subtitle: "registered, quiescent",
        },
        Attached: {
          x: 520,
          y: 248,
          w: 320,
          h: 116,
          subtitle: "runtime bound / ingress armed",
        },
        Running: {
          x: 830,
          y: 392,
          w: 410,
          h: 188,
          subtitle: "active work / interrupts / staged visibility",
          emphasis: true,
        },
        Recovering: {
          x: 1290,
          y: 180,
          w: 320,
          h: 94,
          subtitle: "fault repair / boundary salvage",
        },
        Retired: {
          x: 1390,
          y: 430,
          w: 280,
          h: 96,
          subtitle: "retired / no new work",
        },
        Stopped: {
          x: 1300,
          y: 790,
          w: 300,
          h: 94,
          subtitle: "executor halted / session remains",
        },
        Destroyed: {
          x: 860,
          y: 1060,
          w: 330,
          h: 112,
          subtitle: "terminal sink",
          sink: true,
        },
      },
      statePanels: {
        left: [
          {
            title: "Runtime Binding Plane",
            note: "Identity, work ownership, and the bound runtime envelope.",
            fields: [
              "session_id",
              "active_runtime_id",
              "active_fence_token",
              "active_generation",
              "active_work_id",
            ],
          },
          {
            title: "Control Fabric",
            note: "Wake, process, interrupt, shutdown, ingress, and drain latches.",
            fields: [
              "wake_pending",
              "process_pending",
              "interrupt_pending",
              "shutdown_pending",
              "peer_ingress_configured",
              "drain_running",
            ],
          },
        ],
        right: [
          {
            title: "Visibility Commit Pipeline",
            note: "Staged → active → committed filter promotion with witness tables.",
            fields: [
              "inherited_base_filter",
              "active_filter",
              "staged_filter",
              "active_requested_deferred_names",
              "staged_requested_deferred_names",
              "requested_witnesses",
              "filter_witnesses",
              "active_visibility_revision",
              "staged_visibility_revision",
              "committed_visibility_revision",
            ],
          },
          {
            title: "Peer Reachability Fabric",
            note: "Resolved peers, reachability table, and terminal send reasons.",
            fields: [
              "resolved_peer_keys",
              "peer_reachability",
              "peer_last_reason",
            ],
          },
        ],
      },
      groups: [
        {
          id: "lifecycle",
          title: "Lifecycle + Binding Spine",
          note: "The top-level movement between initialization, registration, binding, recovery, retirement, and destruction.",
          x: 36,
          y: 64,
          w: 430,
          columns: 1,
          anchors: ["Initializing", "Idle", "Attached", "Recovering", "Retired", "Stopped", "Destroyed"],
          triggers: [
            "Initialize",
            "RegisterSession",
            "UnregisterSession",
            "PrepareBindings",
            "Recover",
            "Retire",
            "Reset",
            "StopRuntimeExecutor",
            "Destroy",
            "Recycle",
          ],
        },
        {
          id: "run-boundary",
          title: "Run + Boundary Fabric",
          note: "Interrupt, defer-to-boundary cancellation, terminal work outcomes, and cross-kernel work ingress.",
          x: 620,
          y: 112,
          w: 620,
          columns: 2,
          anchors: ["Running", "Attached"],
          triggers: [
            "InterruptCurrentRun",
            "CancelAfterBoundary",
            "BoundaryApplied",
            "RunCompleted",
            "RunFailed",
            "RunCancelled",
            "SubmitMobWork",
          ],
        },
        {
          id: "visibility",
          title: "Visibility Revision Pipeline",
          note: "Filter staging, deferred tools, boundary promotion, and surface delta handling.",
          x: 1420,
          y: 42,
          w: 500,
          columns: 2,
          anchors: ["Attached", "Running"],
          triggers: [
            "StagePersistentFilter",
            "RequestDeferredTools",
            "PublishCommittedVisibleSet",
            "SurfaceStageAdd",
            "SurfaceStageRemove",
            "SurfaceStageReload",
            "SurfaceApplyBoundary",
            "SurfaceMarkPendingSucceeded",
            "SurfaceMarkPendingFailed",
            "SurfaceCallStarted",
            "SurfaceCallFinished",
            "SurfaceFinalizeRemovalClean",
            "SurfaceFinalizeRemovalForced",
            "SurfaceSnapshotAligned",
            "SurfaceShutdown",
          ],
        },
        {
          id: "peer-drain",
          title: "Peer + Drain Interconnect",
          note: "Ingress context, drain liveness, reachability repair, and session-local drain control.",
          x: 1450,
          y: 430,
          w: 470,
          columns: 2,
          anchors: ["Attached", "Running", "Stopped"],
          triggers: [
            "SetPeerIngressContext",
            "NotifyDrainExited",
            "ReconcileResolvedDirectory",
            "RecordSendSucceeded",
            "RecordSendFailed",
            "Abort",
            "AbortAll",
            "Wait",
            "EnsureDrainRunning",
            "PeerReady",
            "RegisterWatcher",
            "ProgressReported",
          ],
        },
        {
          id: "query-ingress",
          title: "Ingress + Observation Surface",
          note: "Session queries, ingress acceptance, event publication, and boundary receipt inspection.",
          x: 42,
          y: 470,
          w: 450,
          columns: 1,
          anchors: ["Idle", "Attached", "Running"],
          triggers: [
            "EnsureSessionWithExecutor",
            "SetSilentIntents",
            "ContainsSession",
            "SessionHasExecutor",
            "SessionHasComms",
            "OpsLifecycleRegistry",
            "InputState",
            "ListActiveInputs",
            "Ingest",
            "PublishEvent",
            "RuntimeState",
            "LoadBoundaryReceipt",
            "AcceptWithCompletion",
            "AcceptWithoutWake",
          ],
        },
        {
          id: "execution",
          title: "Execution + LLM Return Fabric",
          note: "Prepare/commit/fail plus admission, primitive execution, tool-call return, and boundary exit paths.",
          x: 72,
          y: 810,
          w: 610,
          columns: 2,
          anchors: ["Running", "Attached"],
          triggers: [
            "Prepare",
            "Commit",
            "Fail",
            "AdmitQueued",
            "AdmitConsumedOnAccept",
            "StageDrainSnapshot",
            "SupersedeQueuedInput",
            "CoalesceQueuedInputs",
            "SetSilentIntentOverrides",
            "StartConversationRun",
            "StartImmediateAppend",
            "StartImmediateContext",
            "PrimitiveApplied",
            "LlmReturnedToolCalls",
            "LlmReturnedTerminal",
            "RegisterPendingOps",
            "ToolCallsResolved",
            "OpsBarrierSatisfied",
            "BoundaryContinue",
            "BoundaryComplete",
            "RecoverableFailure",
            "FatalFailure",
            "RetryRequested",
            "CancelNow",
            "CancellationObserved",
            "AcknowledgeTerminal",
          ],
        },
        {
          id: "terminal",
          title: "Terminal + Extraction Control",
          note: "Extraction branch, op provisioning, retirement, wait-all, and terminal collection.",
          x: 730,
          y: 840,
          w: 560,
          columns: 2,
          anchors: ["Running", "Retired", "Stopped"],
          triggers: [
            "TurnLimitReached",
            "BudgetExhausted",
            "TimeBudgetExceeded",
            "EnterExtraction",
            "ExtractionValidationPassed",
            "ExtractionValidationFailed",
            "ExtractionStart",
            "ForceCancelNoRun",
            "RegisterOperation",
            "ProvisioningSucceeded",
            "ProvisioningFailed",
            "AbortProvisioning",
            "CompleteOperation",
            "FailOperation",
            "CancelOperation",
            "RetireRequested",
            "RetireCompleted",
            "CollectTerminal",
            "BeginWaitAll",
            "CancelWaitAll",
            "ClassifyExternalEnvelope",
            "ClassifyPlainEvent",
          ],
        },
      ],
    },
  },
  {
    id: "mob_machine",
    title: "MobMachine",
    subtitle: "Identity / Runtime / Flow / Orchestration Architecture",
    tlaPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "mob_machine",
      "model.tla",
    ),
    contractPath: path.join(
      repoRoot,
      "specs",
      "machines",
      "mob_machine",
      "contract.md",
    ),
    catalogPath: path.join(
      repoRoot,
      "meerkat-machine-schema",
      "src",
      "catalog",
      "dsl",
      "mob_machine.rs",
    ),
    accent: "#6d8fa0",
    layout: {
      plate: { width: 1880, height: 1220 },
      phases: {
        Creating: {
          x: 330,
          y: 104,
          w: 300,
          h: 90,
          subtitle: "boot / empty registry",
        },
        Running: {
          x: 760,
          y: 350,
          w: 420,
          h: 190,
          subtitle: "active members / runs / loops / subscriptions",
          emphasis: true,
        },
        Stopped: {
          x: 1210,
          y: 790,
          w: 300,
          h: 92,
          subtitle: "runtime stopped / state retained",
        },
        Completed: {
          x: 1430,
          y: 530,
          w: 290,
          h: 92,
          subtitle: "mob terminalized",
        },
        Destroyed: {
          x: 800,
          y: 1040,
          w: 340,
          h: 110,
          subtitle: "terminal sink",
          sink: true,
        },
      },
      statePanels: {
        left: [
          {
            title: "Identity Runtime Plane",
            note: "Stable identity, active runtime incarnation, fence, generation, and inflight work.",
            fields: [
              "active_identity",
              "active_runtime_id",
              "active_fence_token",
              "current_generation",
              "inflight_work_id",
            ],
          },
          {
            title: "Operator + Orchestrator Counters",
            note: "Membership, spawn pressure, retirement pressure, and kickoff intent.",
            fields: [
              "active_member_count",
              "pending_spawn_count",
              "retiring_member_count",
              "coordinator_bound",
              "kickoff_pending",
            ],
          },
        ],
        right: [
          {
            title: "Execution Fabric",
            note: "Run, frame, loop, and wiring occupancy inside the orchestration core.",
            fields: [
              "active_run_count",
              "active_frame_count",
              "active_loop_count",
              "wiring_edge_count",
            ],
          },
          {
            title: "Task + Event Surface",
            note: "Task board volume and event subscription pressure.",
            fields: ["task_count", "event_subscription_count"],
          },
        ],
      },
      groups: [
        {
          id: "lifecycle-ops",
          title: "Lifecycle + Operator Control",
          note: "Direct operator/runtime commands that alter mob lifecycle and runtime ownership.",
          x: 32,
          y: 60,
          w: 430,
          columns: 1,
          anchors: ["Creating", "Running", "Stopped", "Completed", "Destroyed"],
          triggers: [
            "Start",
            "Spawn",
            "Retire",
            "Respawn",
            "RetireAll",
            "Stop",
            "Resume",
            "Complete",
            "Reset",
            "Destroy",
            "Shutdown",
            "ForceCancel",
          ],
        },
        {
          id: "runtime-bridge",
          title: "Runtime Bridge Observations",
          note: "Seam-fed runtime notices and member-lifecycle proxy signals that keep the mob model honest.",
          x: 520,
          y: 62,
          w: 620,
          columns: 2,
          anchors: ["Running", "Stopped", "Completed", "Destroyed"],
          triggers: [
            "ObserveRuntimeReady",
            "SubmitWork",
            "CancelWork",
            "CancelAllWork",
            "ObserveWorkCompleted",
            "ObserveWorkFailed",
            "ObserveWorkCancelled",
            "RetireMember",
            "ObserveRuntimeRetired",
            "ResetMember",
            "RespawnMember",
            "DestroyMob",
            "ObserveRuntimeDestroyed",
          ],
        },
        {
          id: "orchestrator",
          title: "Orchestrator Control Matrix",
          note: "Coordinator lifecycle, kickoff choreography, peer exposure, and runtime stop/failure handling.",
          x: 1340,
          y: 58,
          w: 500,
          columns: 2,
          anchors: ["Running", "Completed", "Destroyed"],
          triggers: [
            "MarkCompleted",
            "BeginCleanup",
            "FinishCleanup",
            "InitializeOrchestrator",
            "BindCoordinator",
            "UnbindCoordinator",
            "StageSpawn",
            "CompleteSpawn",
            "StopOrchestrator",
            "ResumeOrchestrator",
            "DestroyOrchestrator",
            "ForceCancelMember",
            "MemberPeerExposed",
            "MemberTerminalized",
            "OperationPeerTrusted",
            "PeerInputAdmitted",
            "RuntimeWorkAdmitted",
            "KickoffStarted",
            "KickoffCallbackPending",
            "KickoffFailed",
            "KickoffCancelled",
            "KickoffForceCancelled",
            "RuntimeRunSubmitted",
            "RuntimeRunCompleted",
            "RuntimeRunFailed",
            "RuntimeRunCancelled",
            "RuntimeStopRequested",
          ],
        },
        {
          id: "topology-turns",
          title: "Topology + Turn Surface",
          note: "Wiring graph changes, external/internal turns, and spawn policy.",
          x: 34,
          y: 470,
          w: 430,
          columns: 1,
          anchors: ["Running", "Creating"],
          triggers: [
            "Wire",
            "Unwire",
            "ExternalTurn",
            "InternalTurn",
            "SetSpawnPolicy",
          ],
        },
        {
          id: "event-surface",
          title: "Event Surface",
          note: "Query/control slab for rosters, streams, snapshots, and provenance recording.",
          x: 1380,
          y: 430,
          w: 460,
          columns: 2,
          anchors: ["Running"],
          triggers: [
            "McpServerStates",
            "RosterSnapshot",
            "ListMembers",
            "ListMembersIncludingRetiring",
            "ListAllMembers",
            "MemberStatus",
            "SubscribeAgentEvents",
            "SubscribeAllAgentEvents",
            "SubscribeMobEvents",
            "PollEvents",
            "ReplayAllEvents",
            "RecordOperatorActionProvenance",
            "GetMember",
            "KickoffBarrierSnapshot",
          ],
        },
        {
          id: "flow-engine",
          title: "Flow Engine Plate",
          note: "Run creation, dispatch, target outcomes, and step-level control semantics.",
          x: 430,
          y: 760,
          w: 630,
          columns: 2,
          anchors: ["Running"],
          triggers: [
            "RunFlow",
            "CancelFlow",
            "FlowStatus",
            "StartFlow",
            "CompleteFlow",
            "CreateRun",
            "StartRun",
            "FinishRun",
            "DispatchStep",
            "CompleteStep",
            "RecordStepOutput",
            "ConditionPassed",
            "ConditionRejected",
            "FailStep",
            "SkipStep",
            "ProjectFrameStepStatus",
            "CancelStep",
            "RegisterTargets",
            "RecordTargetSuccess",
            "RecordTargetTerminalFailure",
            "RecordTargetCanceled",
            "RecordTargetFailure",
          ],
        },
        {
          id: "frame-loop",
          title: "Frame + Loop Fabric",
          note: "Ready/pending frame queues, root/body frame lifecycles, node terminals, and loop control.",
          x: 1080,
          y: 820,
          w: 760,
          columns: 2,
          anchors: ["Running", "Completed"],
          triggers: [
            "RegisterReadyFrame",
            "RegisterPendingBodyFrame",
            "NodeExecutionReleased",
            "FrameTerminated",
            "TerminalizeCompleted",
            "TerminalizeFailed",
            "TerminalizeCanceled",
            "StartRootFrame",
            "StartBodyFrame",
            "CompleteNode",
            "RecordNodeOutput",
            "FailNode",
            "SkipNode",
            "CancelNode",
            "StartLoop",
            "BodyFrameStarted",
            "BodyFrameCompleted",
            "BodyFrameFailed",
            "BodyFrameCanceled",
            "UntilConditionMet",
            "UntilConditionFailed",
            "CancelLoop",
          ],
        },
      ],
    },
  },
];

main();

function main() {
  fs.mkdirSync(outputDir, { recursive: true });

  const posters = MACHINE_SPECS.map((spec) => buildPoster(spec));
  for (const poster of posters) {
    fs.writeFileSync(
      path.join(outputDir, `${poster.id}.html`),
      cleanGeneratedText(renderPosterHtml(poster)),
      "utf8",
    );
  }
  fs.writeFileSync(
    path.join(outputDir, "index.html"),
    cleanGeneratedText(renderIndexHtml(posters)),
    "utf8",
  );

  for (const poster of posters) {
    console.log(
      `generated ${path.relative(repoRoot, path.join(outputDir, `${poster.id}.html`))}`,
    );
  }
  console.log(`generated ${path.relative(repoRoot, path.join(outputDir, "index.html"))}`);
}

function buildPoster(spec) {
  const contract = parseContract(fs.readFileSync(spec.contractPath, "utf8"));
  const tla = parseTla(fs.readFileSync(spec.tlaPath, "utf8"));
  const effectDispositions = parseEffectDispositions(
    fs.readFileSync(spec.catalogPath, "utf8"),
  );
  const triggerRecords = aggregateTriggers(contract, tla);
  const groupedTriggers = buildTriggerGroups(
    spec.layout.groups,
    triggerRecords,
    contract.phases,
  );
  const statePanels = buildStatePanels(spec.layout.statePanels, contract.stateFields);
  const effectBus = buildEffectBus(contract.effects, effectDispositions);
  const invariants = contract.invariants.map((name) => ({
    name,
    formula: tla.invariants.get(name) ?? "—",
  }));
  const phaseStats = buildPhaseStats(contract.phases, contract.transitions, contract.inputNames);
  const schematicSvg = renderSchematicSvg(spec.layout, groupedTriggers, phaseStats, spec.accent);

  return {
    ...spec,
    generatedAt: new Date().toISOString(),
    contract,
    tla,
    triggerRecords,
    groupedTriggers,
    statePanels,
    effectBus,
    invariants,
    phaseStats,
    schematicSvg,
  };
}

function parseContract(text) {
  const lines = text.split(/\r?\n/);
  let section = "";
  let index = 0;
  const stateFields = [];
  const inputs = [];
  const signals = [];
  const effects = [];
  const invariants = [];
  const transitions = [];
  let phases = [];

  while (index < lines.length) {
    const line = lines[index];
    if (line.startsWith("## ")) {
      section = line.slice(3).trim();
      index += 1;
      continue;
    }

    if (section === "State") {
      if (line.startsWith("- Phase enum:")) {
        phases = extractBackticked(line)[0]
          .split("|")
          .map((item) => item.trim())
          .filter(Boolean);
      } else if (line.startsWith("- `")) {
        const match = line.match(/^- `([^`]+)`: `([^`]+)`/);
        if (match) {
          stateFields.push({ name: match[1], type: match[2] });
        }
      }
    } else if (section === "Inputs") {
      const item = parseSignatureBullet(line);
      if (item) inputs.push(item);
    } else if (section === "Signals") {
      const item = parseSignatureBullet(line);
      if (item) signals.push(item);
    } else if (section === "Effects") {
      const item = parseSignatureBullet(line);
      if (item) effects.push(item);
    } else if (section === "Invariants") {
      const item = parseSignatureBullet(line);
      if (item) invariants.push(item.name);
    } else if (section === "Transitions" && line.startsWith("### `")) {
      const [transition, nextIndex] = parseTransitionBlock(lines, index);
      transitions.push(transition);
      index = nextIndex;
      continue;
    }

    index += 1;
  }

  return {
    phases,
    stateFields,
    inputs,
    signals,
    effects,
    invariants,
    transitions,
    inputNames: new Set(inputs.map((item) => item.name)),
    signalNames: new Set(signals.map((item) => item.name)),
  };
}

function parseTransitionBlock(lines, start) {
  const name = lines[start].match(/^### `([^`]+)`$/)?.[1] ?? "Unknown";
  const transition = {
    name,
    from: [],
    onName: "",
    onSignature: "",
    onArgs: [],
    to: "",
    emits: [],
    guards: [],
  };

  let index = start + 1;
  while (index < lines.length) {
    const line = lines[index];
    if (line.startsWith("### `") || line.startsWith("## ")) {
      break;
    }
    if (line.startsWith("- From:")) {
      transition.from = extractBackticked(line);
    } else if (line.startsWith("- On:")) {
      const match = line.match(/^- On: `([^`]+)`(?:\((.*)\))?/);
      if (match) {
        transition.onName = match[1];
        transition.onSignature = `${match[1]}(${match[2] ?? ""})`;
        transition.onArgs = match[2]
          ? match[2]
              .split(",")
              .map((part) => part.trim())
              .filter(Boolean)
          : [];
      }
    } else if (line.startsWith("- Emits:")) {
      transition.emits = extractBackticked(line);
    } else if (line.startsWith("- To:")) {
      transition.to = extractBackticked(line)[0] ?? "";
    } else if (line.startsWith("- Guards:")) {
      index += 1;
      while (index < lines.length && lines[index].startsWith("  - ")) {
        const guardMatch = lines[index].match(/^  - `([^`]+)`$/);
        transition.guards.push(
          guardMatch ? guardMatch[1] : lines[index].replace(/^  - /, "").trim(),
        );
        index += 1;
      }
      continue;
    }
    index += 1;
  }

  return [transition, index];
}

function parseTla(text) {
  const lines = text.split(/\r?\n/);
  const variableLine = lines.find((line) => line.startsWith("VARIABLES ")) ?? "";
  const variables = variableLine
    .replace(/^VARIABLES /, "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);

  const definitions = new Map();
  const defIndices = [];
  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(/^([A-Za-z][A-Za-z0-9_]*)(?:\((.*?)\))? ==$/);
    if (match) {
      defIndices.push({
        index,
        name: match[1],
        params: match[2]
          ? match[2]
              .split(",")
              .map((part) => part.trim())
              .filter(Boolean)
          : [],
      });
    }
  }
  for (let index = 0; index < defIndices.length; index += 1) {
    const current = defIndices[index];
    const end = index + 1 < defIndices.length ? defIndices[index + 1].index : lines.length;
    definitions.set(current.name, {
      params: current.params,
      body: lines.slice(current.index + 1, end).join("\n").trimEnd(),
    });
  }

  const invariants = new Map();
  const theoremStart = lines.findIndex((line) => line.startsWith("THEOREM "));
  const invariantZone = theoremStart >= 0 ? lines.slice(0, theoremStart) : lines;
  for (const line of invariantZone) {
    const match = line.match(/^([a-z_][A-Za-z0-9_]*) == (.*)$/);
    if (match) {
      invariants.set(match[1], match[2]);
    }
  }

  return { variables, definitions, invariants };
}

function parseEffectDispositions(text) {
  const effectMap = new Map();
  for (const match of text.matchAll(/\blocal_disposition\("([^"]+)"\)/g)) {
    effectMap.set(match[1], { kind: "local", consumers: [] });
  }
  for (const match of text.matchAll(/\bexternal_disposition\("([^"]+)"\)/g)) {
    effectMap.set(match[1], { kind: "external", consumers: [] });
  }
  for (const match of text.matchAll(/\brouted_disposition\("([^"]+)",\s*&\[(.*?)\]\)/gs)) {
    const consumers = [...match[2].matchAll(/"([^"]+)"/g)].map((item) => item[1]);
    effectMap.set(match[1], { kind: "routed", consumers });
  }
  return effectMap;
}

function aggregateTriggers(contract, tla) {
  const grouped = new Map();
  for (const transition of contract.transitions) {
    const tlaDef = tla.definitions.get(transition.name);
    const record =
      grouped.get(transition.onName) ??
      {
        name: transition.onName,
        kind: contract.inputNames.has(transition.onName) ? "input" : "signal",
        transitions: [],
        from: new Set(),
        to: new Set(),
        effects: new Set(),
        guards: new Set(),
        writes: new Set(),
      };
    record.transitions.push({
      name: transition.name,
      from: transition.from,
      to: transition.to,
      emits: transition.emits,
      guards: transition.guards,
      signature: transition.onSignature,
    });
    for (const phase of transition.from) record.from.add(phase);
    if (transition.to) record.to.add(transition.to);
    for (const effect of transition.emits) record.effects.add(effect);
    for (const guard of transition.guards) record.guards.add(guard);
    if (tlaDef) {
      for (const write of extractWrites(tlaDef.body)) {
        if (write !== "model_step_count") {
          record.writes.add(write);
        }
      }
    }
    grouped.set(transition.onName, record);
  }

  return [...grouped.values()]
    .map((record) => ({
      name: record.name,
      kind: record.kind,
      transitions: record.transitions,
      from: [...record.from],
      to: [...record.to],
      effects: [...record.effects],
      guards: [...record.guards],
      writes: [...record.writes],
      pathLabel: renderTriggerPath(record.transitions),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function buildTriggerGroups(layoutGroups, triggerRecords, phases) {
  const triggerMap = new Map(triggerRecords.map((record) => [record.name, record]));
  const assigned = new Set();
  const groups = [];

  for (const config of layoutGroups) {
    const records = config.triggers
      .map((name) => triggerMap.get(name))
      .filter(Boolean);
    for (const record of records) {
      assigned.add(record.name);
    }
    groups.push(finalizeGroup(config, records, phases));
  }

  const leftovers = triggerRecords.filter((record) => !assigned.has(record.name));
  if (leftovers.length > 0) {
    console.warn(
      `[posters] ${leftovers.length} unassigned triggers fell into Emergent Surfaces: ${leftovers
        .map((item) => item.name)
        .join(", ")}`,
    );
    groups.push(
      finalizeGroup(
        {
          id: "emergent",
          title: "Emergent / Unassigned Surfaces",
          note: "Schema changed without an explicit visual assignment. Review this cluster and reclassify it deliberately.",
          x: 40,
          y: 1120,
          w: 520,
          columns: 2,
          anchors: [phases[phases.length - 1]],
        },
        leftovers,
        phases,
      ),
    );
  }

  return groups;
}

function finalizeGroup(config, records, phaseOrder) {
  const columns = config.columns ?? (records.length > 12 ? 2 : 1);
  const rowCount = Math.max(1, Math.ceil(records.length / columns));
  const height = 88 + rowCount * 34 + 108;
  const inputCount = records.filter((item) => item.kind === "input").length;
  const signalCount = records.length - inputCount;
  const writes = unique(records.flatMap((item) => item.writes));
  const guards = unique(records.flatMap((item) => item.guards));
  const effects = unique(records.flatMap((item) => item.effects));

  return {
    ...config,
    records: records.map((record) => ({
      ...record,
      pathLabel: normalizePhaseList(record.pathLabel, phaseOrder),
    })),
    columns,
    height,
    stats: { total: records.length, inputCount, signalCount },
    writes,
    guards,
    effects,
    tone:
      inputCount === 0 ? "signal" : signalCount === 0 ? "input" : "hybrid",
  };
}

function buildStatePanels(panelConfig, stateFields) {
  const stateFieldMap = new Map(stateFields.map((field) => [field.name, field]));
  return {
    left: buildStatePanelList(panelConfig.left, stateFieldMap),
    right: buildStatePanelList(panelConfig.right, stateFieldMap),
  };
}

function buildStatePanelList(panels, stateFieldMap) {
  const used = new Set();
  const realized = panels.map((panel) => {
    const fields = panel.fields
      .map((name) => stateFieldMap.get(name))
      .filter(Boolean);
    for (const field of fields) used.add(field.name);
    return { ...panel, fields };
  });
  const leftover = [...stateFieldMap.values()].filter((field) => !used.has(field.name));
  if (leftover.length > 0) {
    realized.push({
      title: "Residual Registers",
      note: "Fields not explicitly assigned to a poster subsystem.",
      fields: leftover,
    });
  }
  return realized;
}

function buildEffectBus(effects, dispositionMap) {
  const lanes = {
    routed: [],
    external: [],
    local: [],
    unknown: [],
  };
  for (const effect of effects) {
    const info = dispositionMap.get(effect.name);
    if (!info) {
      lanes.unknown.push({ name: effect.name, consumers: [] });
      continue;
    }
    lanes[info.kind].push({ name: effect.name, consumers: info.consumers });
  }
  return lanes;
}

function buildPhaseStats(phases, transitions, inputNames) {
  const stats = new Map();
  for (const phase of phases) {
    stats.set(phase, {
      loopCount: 0,
      exitCount: 0,
      entryCount: 0,
      inputLegs: 0,
      signalLegs: 0,
    });
  }

  for (const transition of transitions) {
    const isInput = inputNames.has(transition.onName);
    for (const from of transition.from) {
      const phaseStats = stats.get(from);
      if (transition.to === from) {
        phaseStats.loopCount += 1;
      } else {
        phaseStats.exitCount += 1;
      }
      if (isInput) {
        phaseStats.inputLegs += 1;
      } else {
        phaseStats.signalLegs += 1;
      }
    }
    if (transition.to && !transition.from.includes(transition.to)) {
      stats.get(transition.to).entryCount += 1;
    }
  }
  return stats;
}

function renderPosterHtml(poster) {
  const outerPad = 24;
  const leftRail = 304;
  const rightRail = 316;
  const railGap = 18;
  const headerHeight = 172;
  const footerHeight = 262;
  const canvasWidth =
    outerPad * 2 + leftRail + railGap + poster.layout.plate.width + railGap + rightRail;
  const canvasHeight = headerHeight + poster.layout.plate.height + footerHeight;
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>${escapeHtml(poster.title)} architectural plate</title>
    <style>${renderStyles(poster)}</style>
  </head>
  <body>
    <main class="page-shell">
      <a class="page-index-link" href="./index.html">← Poster index</a>
      <article class="architectural-plate">
        <svg class="poster-svg" viewBox="0 0 ${canvasWidth} ${canvasHeight}" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="${escapeHtml(
          poster.title,
        )} architectural plate">
          ${renderPosterPlateSvg(poster, {
            outerPad,
            leftRail,
            rightRail,
            railGap,
            headerHeight,
            footerHeight,
            canvasWidth,
            canvasHeight,
          })}
        </svg>
      </article>
    </main>
  </body>
</html>`;
}

function renderPosterPlateSvg(poster, dims) {
  const { outerPad, leftRail, rightRail, railGap, headerHeight, canvasWidth, canvasHeight } = dims;
  const plateX = outerPad + leftRail + railGap;
  const plateY = headerHeight;
  const plateW = poster.layout.plate.width;
  const plateH = poster.layout.plate.height;
  const leftX = outerPad;
  const rightX = canvasWidth - outerPad - rightRail;
  const bottomY = plateY + plateH + 26;
  const effectW = 1020;
  const invariantW = canvasWidth - outerPad - (leftX + effectW + 22);

  return `
    <defs>
      <linearGradient id="pageFade" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0%" stop-color="rgba(255,255,255,0.03)" />
        <stop offset="100%" stop-color="rgba(255,255,255,0.01)" />
      </linearGradient>
      <marker id="plateArrow" markerWidth="14" markerHeight="14" refX="11" refY="7" orient="auto">
        <path d="M0,0 L14,7 L0,14 z" fill="${poster.accent}" />
      </marker>
      <marker id="ghostArrow" markerWidth="12" markerHeight="12" refX="9" refY="6" orient="auto">
        <path d="M0,0 L12,6 L0,12 z" fill="rgba(244,239,229,0.3)" />
      </marker>
    </defs>
    <rect x="0" y="0" width="${canvasWidth}" height="${canvasHeight}" rx="28" fill="${THEME.shell}" stroke="rgba(255,255,255,0.12)" />
    <rect x="1" y="1" width="${canvasWidth - 2}" height="${canvasHeight - 2}" rx="27" fill="url(#pageFade)" opacity="0.9" />
    ${renderSvgHeader(poster, canvasWidth)}
    ${renderSvgMetricRack(poster, 48, 132)}
    ${renderSvgPanelStack(poster.statePanels.left, leftX, plateY + 26, leftRail)}
    ${renderSvgPanelStack(poster.statePanels.right, rightX, plateY + 26, rightRail)}
    <g transform="translate(${plateX}, ${plateY})">
      ${renderSchematicSvg(poster.layout, poster.groupedTriggers, poster.phaseStats, poster.accent)}
      ${renderSvgPhaseCards(poster)}
      ${poster.groupedTriggers.map((group) => renderSvgGroupCard(group)).join("")}
    </g>
    ${renderSvgEffectBus(poster.effectBus, leftX, bottomY, effectW)}
    ${renderSvgInvariantPlate(poster.invariants, leftX + effectW + 22, bottomY, invariantW)}
    <text x="${canvasWidth - 48}" y="${canvasHeight - 18}" text-anchor="end" fill="${THEME.faint}" font-size="10" letter-spacing="2" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">REGENERATE WITH NODE SCRIPTS/MACHINE-POSTERS/GENERATE-MACHINE-POSTERS.MJS</text>
  `;
}

function renderSvgHeader(poster, canvasWidth) {
  const titleX = 48;
  const plaqueX = canvasWidth - 520;
  const meta = [
    ["TLA", path.relative(repoRoot, poster.tlaPath)],
    ["contract", path.relative(repoRoot, poster.contractPath)],
    ["catalog", path.relative(repoRoot, poster.catalogPath)],
    ["generated", poster.generatedAt.replace("T", " ").replace("Z", " UTC")],
  ];
  return `
    <text x="${titleX}" y="44" fill="${THEME.faint}" font-size="11" letter-spacing="3" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">CANONICAL MACHINE PLATE / GENERATED FROM TLA + CONTRACT ARTIFACTS</text>
    <text x="${titleX}" y="106" fill="${THEME.ink}" font-size="66" font-weight="600" letter-spacing="-5" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${escapeHtml(
      poster.title,
    )}</text>
    <text x="${titleX}" y="140" fill="${THEME.muted}" font-size="16" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${escapeHtml(
      poster.subtitle,
    )}</text>
    <g transform="translate(${plaqueX}, 26)">
      <rect x="0" y="0" width="472" height="112" rx="16" fill="${THEME.panel}" stroke="rgba(255,255,255,0.09)" />
      ${meta
        .map((item, index) => {
          const x = index % 2 === 0 ? 14 : 238;
          const y = 14 + Math.floor(index / 2) * 40;
          return `
            <rect x="${x}" y="${y}" width="220" height="28" rx="8" fill="rgba(255,255,255,0.02)" stroke="rgba(255,255,255,0.06)" />
            <text x="${x + 10}" y="${y + 10}" fill="${THEME.faint}" font-size="9" letter-spacing="1.5" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${escapeHtml(
              item[0].toUpperCase(),
            )}</text>
            <text x="${x + 10}" y="${y + 22}" fill="${THEME.ink}" font-size="10" font-family="ui-monospace, SFMono-Regular, Menlo, monospace">${escapeHtml(
              item[1],
            )}</text>`;
        })
        .join("")}
      <g transform="translate(14, 96)">
        ${[
          ["INPUT", THEME.input],
          ["SIGNAL", THEME.signal],
          ["EFFECT", THEME.effect],
        ]
          .map(
            ([label, color], index) => `
              <rect x="${index * 148}" y="-10" width="136" height="16" rx="8" fill="rgba(255,255,255,0.02)" stroke="rgba(255,255,255,0.05)" />
              <rect x="${index * 148 + 8}" y="-5" width="8" height="8" rx="2" fill="${color}" />
              <text x="${index * 148 + 24}" y="2" fill="${THEME.ink}" font-size="10" letter-spacing="1.6" font-weight="600" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${label}</text>`,
          )
          .join("")}
      </g>
    </g>
  `;
}

function renderSvgMetricRack(poster, x, y) {
  const items = [
    ["PHASES", poster.contract.phases.length],
    ["DOMAINS", poster.groupedTriggers.length],
    ["INPUTS", poster.contract.inputs.length],
    ["SIGNALS", poster.contract.signals.length],
    ["EFFECTS", poster.contract.effects.length],
    ["INVARIANTS", poster.invariants.length],
    ["STATE", poster.contract.stateFields.length],
  ];
  return `<g transform="translate(${x}, ${y})">
    ${items
      .map(
        ([label, value], index) => `
          <rect x="${index * 62}" y="0" width="54" height="28" rx="8" fill="rgba(255,255,255,0.03)" stroke="rgba(255,255,255,0.07)" />
          <text x="${index * 62 + 8}" y="10" fill="${THEME.faint}" font-size="8" letter-spacing="1.4" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${label}</text>
          <text x="${index * 62 + 8}" y="22" fill="${THEME.ink}" font-size="11" font-weight="600" font-family="ui-monospace, SFMono-Regular, Menlo, monospace">${value}</text>`,
      )
      .join("")}
  </g>`;
}

function renderSvgPanelStack(panels, x, startY, width) {
  let y = startY;
  return panels
    .map((panel) => {
      const columns = 2;
      const cellW = Math.floor((width - 18 - 8) / 2);
      const rows = Math.max(1, Math.ceil(panel.fields.length / columns));
      const height = 34 + rows * 34 + 14;
      const svg = `<g transform="translate(${x}, ${y})">
        <rect x="0" y="0" width="${width}" height="${height}" rx="12" fill="${THEME.panel}" stroke="rgba(255,255,255,0.08)" />
        <text x="12" y="14" fill="${THEME.faint}" font-size="8" letter-spacing="1.5" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">SUBSYSTEM</text>
        <text x="12" y="28" fill="${THEME.ink}" font-size="13" font-weight="600" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${escapeHtml(
          panel.title,
        )}</text>
        ${panel.fields
          .map((field, index) => {
            const col = index % columns;
            const row = Math.floor(index / columns);
            const cellX = 10 + col * (cellW + 8);
            const cellY = 40 + row * 34;
            return `
              <rect x="${cellX}" y="${cellY}" width="${cellW}" height="26" rx="7" fill="rgba(255,255,255,0.03)" stroke="rgba(255,255,255,0.05)" />
              <text x="${cellX + 7}" y="${cellY + 16}" fill="${THEME.ink}" font-size="9" font-family="ui-monospace, SFMono-Regular, Menlo, monospace">${escapeHtml(
                field.name,
              )}</text>`;
          })
          .join("")}
      </g>`;
      y += height + 12;
      return svg;
    })
    .join("");
}

function renderSvgPhaseCards(poster) {
  return poster.contract.phases
    .map((phase) => {
      const layout = poster.layout.phases[phase];
      const stats = poster.phaseStats.get(phase);
      const fill = layout.sink
        ? "rgba(68,18,18,0.88)"
        : layout.emphasis
          ? "rgba(201,109,84,0.18)"
          : "rgba(132,191,215,0.08)";
      const stroke = layout.sink
        ? "rgba(201,109,84,0.55)"
        : layout.emphasis
          ? "rgba(201,109,84,0.42)"
          : "rgba(255,255,255,0.12)";
      return `
        <g transform="translate(${layout.x}, ${layout.y})">
          <rect x="0" y="0" width="${layout.w}" height="${layout.h}" rx="14" fill="${fill}" stroke="${stroke}" />
          <text x="16" y="16" fill="${THEME.faint}" font-size="8" letter-spacing="1.8" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">PHASE</text>
          <text x="16" y="42" fill="${THEME.ink}" font-size="${layout.emphasis ? 32 : 24}" font-weight="600" letter-spacing="-1.5" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${escapeHtml(
            phase,
          )}</text>
          <text x="16" y="${layout.h - 22}" fill="${THEME.muted}" font-size="9" letter-spacing="1" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">L:${stats.loopCount} E:${stats.exitCount} I:${stats.entryCount}</text>
        </g>`;
    })
    .join("");
}

function renderSvgGroupCard(group) {
  const cols = group.columns;
  const innerX = 12;
  const headerH = 34;
  const gap = 6;
  const cellW = Math.floor((group.w - innerX * 2 - gap * (cols - 1)) / cols);
  const cellH = 18;
  return `<g transform="translate(${group.x}, ${group.y})">
    <rect x="0" y="0" width="${group.w}" height="${group.height}" rx="14" fill="rgba(20,26,31,0.82)" stroke="${
      group.tone === "input" ? THEME.input : group.tone === "signal" ? THEME.signal : posterAccent(group)
    }" stroke-opacity="0.28" />
    <text x="14" y="15" fill="${THEME.faint}" font-size="8" letter-spacing="1.8" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">CONTROL DOMAIN</text>
    <text x="14" y="30" fill="${THEME.ink}" font-size="13" font-weight="600" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${escapeHtml(
      group.title,
    )}</text>
    <text x="${group.w - 14}" y="15" text-anchor="end" fill="${THEME.faint}" font-size="8" letter-spacing="1.5" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${group.stats.total} TRIGGERS</text>
    ${group.records
      .map((record, index) => {
        const col = index % cols;
        const row = Math.floor(index / cols);
        const x = innerX + col * (cellW + gap);
        const y = headerH + row * (cellH + 4);
        const color = record.kind === "input" ? THEME.input : THEME.signal;
        return `
          <rect x="${x}" y="${y}" width="${cellW}" height="${cellH}" rx="5" fill="rgba(255,255,255,0.018)" stroke="rgba(255,255,255,0.05)" />
          <rect x="${x + 6}" y="${y + 6}" width="6" height="6" rx="1" fill="${color}" />
          <text x="${x + 18}" y="${y + 12.5}" fill="${THEME.ink}" font-size="8.6" font-family="ui-monospace, SFMono-Regular, Menlo, monospace">${escapeHtml(
            record.name,
          )}</text>`;
      })
      .join("")}
  </g>`;
}

function posterAccent(group) {
  return "rgba(201,109,84,0.28)";
}

function renderSvgEffectBus(effectBus, x, y, width) {
  const laneDefs = [
    ["ROUTED EFFECTS", THEME.routed, effectBus.routed],
    ["EXTERNAL EFFECTS", THEME.external, effectBus.external],
    ["LOCAL EFFECTS", THEME.local, effectBus.local],
  ];
  let offsetY = y;
  return laneDefs
    .map(([title, color, effects]) => {
      const lane = `<g transform="translate(${x}, ${offsetY})">
        <rect x="0" y="0" width="${width}" height="40" rx="10" fill="${THEME.panel}" stroke="rgba(255,255,255,0.08)" />
        <rect x="0" y="0" width="4" height="40" rx="4" fill="${color}" />
        <text x="12" y="13" fill="${THEME.faint}" font-size="8" letter-spacing="1.5" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">${title}</text>
        ${effects
          .slice(0, 12)
          .map((effect, index) => {
            const chipX = 12 + index * 82;
            return `
              <rect x="${chipX}" y="18" width="76" height="14" rx="7" fill="rgba(255,255,255,0.03)" stroke="rgba(255,255,255,0.05)" />
              <text x="${chipX + 6}" y="27.5" fill="${THEME.ink}" font-size="7.4" font-family="ui-monospace, SFMono-Regular, Menlo, monospace">${escapeHtml(
                effect.name,
              )}</text>`;
          })
          .join("")}
      </g>`;
      offsetY += 46;
      return lane;
    })
    .join("");
}

function renderSvgInvariantPlate(invariants, x, y, width) {
  const cols = 2;
  const gap = 8;
  const cellW = Math.floor((width - 18 - gap) / cols);
  return `<g transform="translate(${x}, ${y})">
    <rect x="0" y="0" width="${width}" height="${Math.max(146, 30 + Math.ceil(invariants.length / cols) * 26)}" rx="12" fill="${THEME.panel}" stroke="rgba(255,255,255,0.08)" />
    <text x="12" y="14" fill="${THEME.faint}" font-size="8" letter-spacing="1.5" font-family="Avenir Next, Helvetica Neue, Arial, sans-serif">INVARIANT PLATE</text>
    ${invariants
      .map((inv, index) => {
        const col = index % cols;
        const row = Math.floor(index / cols);
        const cx = 10 + col * (cellW + gap);
        const cy = 22 + row * 26;
        return `
          <rect x="${cx}" y="${cy}" width="${cellW}" height="20" rx="8" fill="rgba(255,255,255,0.03)" stroke="rgba(255,255,255,0.05)" />
          <text x="${cx + 8}" y="${cy + 13}" fill="${THEME.ink}" font-size="8" font-family="ui-monospace, SFMono-Regular, Menlo, monospace">${escapeHtml(
            inv.name,
          )}</text>`;
      })
      .join("")}
  </g>`;
}

function renderSchematicPlate(poster) {
  const { width, height } = poster.layout.plate;
  return `<section class="schematic-plate" style="width:${width}px; height:${height}px;">
    <svg class="schematic-svg" viewBox="0 0 ${width} ${height}" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="${escapeHtml(
      poster.title,
    )} schematic">${poster.schematicSvg}</svg>
    ${renderPhaseCards(poster)}
    ${poster.groupedTriggers.map((group) => renderGroupCard(group)).join("")}
  </section>`;
}

function renderPhaseCards(poster) {
  return poster.contract.phases
    .map((phase) => {
      const layout = poster.layout.phases[phase];
      const stats = poster.phaseStats.get(phase);
      return `<article class="phase-card ${layout.emphasis ? "phase-card--emphasis" : ""} ${
        layout.sink ? "phase-card--sink" : ""
      }" style="left:${layout.x}px; top:${layout.y}px; width:${layout.w}px; height:${layout.h}px;">
        <div class="phase-card__eyebrow">phase</div>
        <h2>${escapeHtml(phase)}</h2>
        <div class="phase-card__subtitle">${escapeHtml(layout.subtitle ?? "")}</div>
        <div class="phase-card__stats">
          <span>loops ${stats.loopCount}</span>
          <span>exits ${stats.exitCount}</span>
          <span>entries ${stats.entryCount}</span>
        </div>
        <div class="phase-card__stats phase-card__stats--secondary">
          <span>input legs ${stats.inputLegs}</span>
          <span>signal legs ${stats.signalLegs}</span>
        </div>
      </article>`;
    })
    .join("");
}

function renderGroupCard(group) {
  return `<section class="cluster cluster--${group.tone}" style="left:${group.x}px; top:${group.y}px; width:${group.w}px; height:${group.height}px; --cols:${group.columns};">
    <header class="cluster__header">
      <div>
        <div class="cluster__eyebrow">control domain</div>
        <h3>${escapeHtml(group.title)}</h3>
      </div>
      <div class="cluster__counts">
        <span>${group.stats.total} triggers</span>
        <span>${group.stats.inputCount} input</span>
        <span>${group.stats.signalCount} signal</span>
      </div>
    </header>
    <div class="cluster__grid">
      ${group.records
        .map(
          (record) => `<div class="trigger-row trigger-row--${record.kind}">
            <span class="trigger-row__kind"></span>
            <div class="trigger-row__body">
              <div class="trigger-row__name">${escapeHtml(record.name)}</div>
              <div class="trigger-row__path">${escapeHtml(record.pathLabel)}</div>
            </div>
          </div>`,
        )
        .join("")}
    </div>
    <footer class="cluster__footer">
      ${renderTokenLane("writes", group.writes, 9)}
      ${renderTokenLane("guards", group.guards, 6)}
      ${renderTokenLane("effects", group.effects, 6)}
    </footer>
  </section>`;
}

function renderStatePanel(panel) {
  return `<section class="state-panel">
    <div class="section-label">Subsystem</div>
    <h3 class="state-panel__title">${escapeHtml(panel.title)}</h3>
    <div class="state-field-list">
      ${panel.fields
        .map(
          (field) => `<div class="state-field">
            <strong>${escapeHtml(field.name)}</strong>
            <span>${escapeHtml(field.type)}</span>
          </div>`,
        )
        .join("")}
    </div>
  </section>`;
}

function renderEffectLane(kind, title, effects) {
  return `<article class="effect-lane effect-lane--${kind}">
    <div class="effect-lane__title">${escapeHtml(title)}</div>
    <div class="effect-chip-list">
      ${effects
        .map((effect) => {
          const label =
            effect.consumers && effect.consumers.length > 0
              ? `${effect.name} → ${effect.consumers.join(", ")}`
              : effect.name;
          return `<span class="effect-chip">${escapeHtml(label)}</span>`;
        })
        .join("")}
    </div>
  </article>`;
}

function renderTokenLane(label, items, limit) {
  if (items.length === 0) {
    return "";
  }
  const shown = items.slice(0, limit);
  const hidden = items.length - shown.length;
  return `<div class="token-lane">
    <span class="token-lane__label">${escapeHtml(label)}</span>
    <div class="token-lane__tokens">
      ${shown.map((item) => `<span class="token-chip">${escapeHtml(item)}</span>`).join("")}
      ${hidden > 0 ? `<span class="token-chip token-chip--overflow">+${hidden}</span>` : ""}
    </div>
  </div>`;
}

function renderMetaChip(label, value) {
  return `<div class="meta-chip"><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`;
}

function renderStatChip(label, value) {
  return `<div class="stat-chip"><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`;
}

function renderSchematicSvg(layout, groups, phaseStats, accent) {
  const { width, height } = layout.plate;
  const phaseEntries = Object.entries(layout.phases).map(([name, box]) => ({
    name,
    ...box,
    cx: box.x + box.w / 2,
    cy: box.y + box.h / 2,
  }));

  const svg = [];
  svg.push(
    `<defs>
      <marker id="phase-arrow" markerWidth="12" markerHeight="12" refX="10" refY="6" orient="auto">
        <path d="M0,0 L12,6 L0,12 z" fill="${accent}" />
      </marker>
      <marker id="ghost-arrow" markerWidth="10" markerHeight="10" refX="8" refY="5" orient="auto">
        <path d="M0,0 L10,5 L0,10 z" fill="rgba(244,239,229,0.28)" />
      </marker>
    </defs>`,
  );
  svg.push(
    `<rect x="0" y="0" width="${width}" height="${height}" fill="${THEME.plate}" stroke="${THEME.border}" rx="28" />`,
  );
  svg.push(renderGrid(width, height));

  const phaseEdgeMap = buildPhaseEdgeMap(phaseEntries.map((item) => item.name), groups);
  for (const edge of phaseEdgeMap) {
    const from = phaseEntries.find((item) => item.name === edge.from);
    const to = phaseEntries.find((item) => item.name === edge.to);
    if (!from || !to || edge.from === edge.to) continue;
    const [sx, sy] = anchorPoint(from, to);
    const [ex, ey] = anchorPoint(to, from);
    const midX = Math.round((sx + ex) / 2);
    const route = `M ${sx} ${sy} L ${midX} ${sy} L ${midX} ${ey} L ${ex} ${ey}`;
    svg.push(
      `<path d="${route}" fill="none" stroke="${accent}" stroke-width="3" stroke-opacity="0.8" marker-end="url(#phase-arrow)" />`,
    );
    svg.push(
      `<text x="${midX + 10}" y="${Math.round((sy + ey) / 2) - 8}" fill="${accent}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="13">${edge.count}</text>`,
    );
  }

  for (const group of groups) {
    const groupBox = {
      x: group.x,
      y: group.y,
      w: group.w,
      h: group.height,
      cx: group.x + group.w / 2,
      cy: group.y + group.height / 2,
    };
    for (const phaseName of group.anchors) {
      const phase = phaseEntries.find((entry) => entry.name === phaseName);
      if (!phase) continue;
      const [sx, sy] = anchorPoint(groupBox, phase);
      const [ex, ey] = anchorPoint(phase, groupBox);
      const horizontal = Math.abs(ex - sx) >= Math.abs(ey - sy);
      const mid = horizontal ? Math.round((sx + ex) / 2) : Math.round((sy + ey) / 2);
      const route = horizontal
        ? `M ${sx} ${sy} L ${mid} ${sy} L ${mid} ${ey} L ${ex} ${ey}`
        : `M ${sx} ${sy} L ${sx} ${mid} L ${ex} ${mid} L ${ex} ${ey}`;
      const stroke =
        group.tone === "input"
          ? THEME.input
          : group.tone === "signal"
            ? THEME.signal
            : "rgba(244,239,229,0.34)";
      svg.push(
        `<path d="${route}" fill="none" stroke="${stroke}" stroke-width="1.8" stroke-dasharray="8 7" marker-end="url(#ghost-arrow)" />`,
      );
    }
  }

  return svg.join("");
}

function buildPhaseEdgeMap(phases, groups) {
  const map = new Map();
  for (const group of groups) {
    for (const record of group.records) {
      for (const transition of record.transitions) {
        for (const from of transition.from) {
          const key = `${from}->${transition.to}`;
          map.set(key, {
            from,
            to: transition.to,
            count: (map.get(key)?.count ?? 0) + 1,
          });
        }
      }
    }
  }
  return [...map.values()].filter((edge) => phases.includes(edge.from) && phases.includes(edge.to));
}

function renderGrid(width, height) {
  const vertical = [];
  const horizontal = [];
  for (let x = 40; x < width; x += 40) {
    vertical.push(`<line x1="${x}" y1="0" x2="${x}" y2="${height}" />`);
  }
  for (let y = 40; y < height; y += 40) {
    horizontal.push(`<line x1="0" y1="${y}" x2="${width}" y2="${y}" />`);
  }
  return `<g stroke="${THEME.grid}" stroke-width="1">${vertical.join("")}${horizontal.join("")}</g>`;
}

function anchorPoint(source, target) {
  const dx = target.cx - source.cx;
  const dy = target.cy - source.cy;
  if (Math.abs(dx) > Math.abs(dy)) {
    return [
      dx >= 0 ? source.x + source.w : source.x,
      clamp(target.cy, source.y + 18, source.y + source.h - 18),
    ];
  }
  return [
    clamp(target.cx, source.x + 18, source.x + source.w - 18),
    dy >= 0 ? source.y + source.h : source.y,
  ];
}

function renderIndexHtml(posters) {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Machine architectural plates</title>
    <style>
      body {
        margin: 0;
        min-height: 100vh;
        background: ${THEME.bg};
        color: ${THEME.ink};
        font-family: "Avenir Next", "Helvetica Neue", Arial, sans-serif;
      }
      main {
        max-width: 1180px;
        margin: 0 auto;
        padding: 54px 32px 80px;
      }
      .eyebrow {
        color: ${THEME.muted};
        letter-spacing: 0.2em;
        text-transform: uppercase;
        font-size: 12px;
      }
      h1 {
        margin: 14px 0 12px;
        font-size: 54px;
        letter-spacing: -0.06em;
      }
      p {
        margin: 0 0 32px;
        max-width: 50rem;
        color: ${THEME.muted};
        line-height: 1.6;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
        gap: 20px;
      }
      .card {
        display: block;
        padding: 28px;
        border-radius: 22px;
        background: ${THEME.panel};
        border: 1px solid ${THEME.border};
        color: inherit;
        text-decoration: none;
        box-shadow: 0 24px 42px rgba(0,0,0,0.25);
      }
      .card h2 {
        margin: 8px 0 10px;
        font-size: 32px;
        letter-spacing: -0.04em;
      }
      .card small {
        display: block;
        margin-top: 16px;
        color: ${THEME.faint};
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
      }
    </style>
  </head>
  <body>
    <main>
      <div class="eyebrow">Generated architectural plates</div>
      <h1>Canonical machine posters</h1>
      <p>
        Large-format, regenerated plates built from the canonical TLA machine models
        and normalized contract metadata. Each poster is laid out as a phase core
        with subsystem halos, sidecar state blocks, and an effect bus.
      </p>
      <div class="grid">
        ${posters
          .map(
            (poster) => `<a class="card" href="./${poster.id}.html">
              <div class="eyebrow">${escapeHtml(poster.subtitle)}</div>
              <h2>${escapeHtml(poster.title)}</h2>
              <p>${poster.contract.inputs.length} inputs · ${poster.contract.signals.length} signals · ${poster.contract.effects.length} effects</p>
              <small>${escapeHtml(path.relative(repoRoot, poster.tlaPath))}</small>
            </a>`,
          )
          .join("")}
      </div>
    </main>
  </body>
</html>`;
}

function renderStyles(poster) {
  return `
    :root {
      --bg: ${THEME.bg};
      --shell: ${THEME.shell};
      --plate: ${THEME.plate};
      --panel: ${THEME.panel};
      --card: ${THEME.card};
      --border: ${THEME.border};
      --grid: ${THEME.grid};
      --ink: ${THEME.ink};
      --muted: ${THEME.muted};
      --faint: ${THEME.faint};
      --input: ${THEME.input};
      --signal: ${THEME.signal};
      --effect: ${THEME.effect};
      --routed: ${THEME.routed};
      --local: ${THEME.local};
      --external: ${THEME.external};
      --accent: ${poster.accent};
      --sink: ${THEME.sink};
    }
    * { box-sizing: border-box; }
    html, body { margin: 0; }
    body {
      background:
        radial-gradient(circle at 20% 0%, rgba(201, 109, 84, 0.08), transparent 30%),
        radial-gradient(circle at 90% 10%, rgba(132, 191, 215, 0.08), transparent 28%),
        linear-gradient(180deg, #090b0d 0%, #0b0d10 18%, #0b0d0f 100%);
      color: var(--ink);
      font-family: "Avenir Next", "Helvetica Neue", Arial, sans-serif;
    }
    code, pre, .mono {
      font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    }
    .page-shell {
      min-height: 100vh;
      padding: 22px 24px 34px;
    }
    .page-index-link {
      display: inline-block;
      margin: 0 0 12px 6px;
      color: var(--accent);
      text-decoration: none;
      font-size: 11px;
      letter-spacing: 0.22em;
      text-transform: uppercase;
    }
    .architectural-plate {
      width: min(3520px, calc(100vw - 48px));
      max-width: 3520px;
      margin: 0 auto;
      padding: 10px;
      background:
        linear-gradient(180deg, rgba(255,255,255,0.02), transparent 24%),
        linear-gradient(90deg, rgba(255,255,255,0.01), transparent 22%, transparent 78%, rgba(255,255,255,0.01)),
        var(--shell);
      border: 1px solid rgba(255,255,255,0.11);
      border-radius: 24px;
      box-shadow:
        0 44px 120px rgba(0,0,0,0.5),
        inset 0 1px 0 rgba(255,255,255,0.05),
        inset 0 0 0 1px rgba(0,0,0,0.22);
    }
    .poster-svg {
      display: block;
      width: 100%;
      height: auto;
      border-radius: 18px;
    }
    .plate-header {
      display: flex;
      justify-content: space-between;
      gap: 18px;
      align-items: flex-end;
      padding: 2px 4px 10px;
      border-bottom: 1px solid rgba(255,255,255,0.08);
    }
    .title-plaque {
      flex: 1 1 auto;
      min-width: 0;
    }
    .eyebrow, .section-label, .cluster__eyebrow, .phase-card__eyebrow {
      color: var(--faint);
      font-size: 11px;
      letter-spacing: 0.2em;
      text-transform: uppercase;
    }
    h1 {
      margin: 6px 0 0;
      font-size: clamp(48px, 3.8vw, 84px);
      line-height: 0.94;
      letter-spacing: -0.07em;
      font-weight: 600;
    }
    .subtitle {
      margin: 8px 0 0;
      color: var(--muted);
      font-size: 14px;
      line-height: 1.35;
      max-width: 44rem;
    }
    .legend-plaque {
      width: 620px;
      display: grid;
      gap: 10px;
      padding: 12px 14px;
      border: 1px solid rgba(255,255,255,0.08);
      border-radius: 18px;
      background: rgba(255,255,255,0.025);
      box-shadow: inset 0 1px 0 rgba(255,255,255,0.04);
    }
    .legend-plaque__section {
      display: grid;
      gap: 10px;
    }
    .meta-grid {
      display: grid;
      grid-template-columns: repeat(2, minmax(220px, 1fr));
      gap: 8px;
      min-width: auto;
    }
    .meta-chip, .stat-chip {
      background: rgba(255,255,255,0.018);
      border: 1px solid rgba(255,255,255,0.08);
      border-radius: 12px;
      padding: 8px 10px;
    }
    .meta-chip span, .stat-chip span {
      display: block;
      color: var(--faint);
      text-transform: uppercase;
      letter-spacing: 0.14em;
      font-size: 10px;
    }
    .meta-chip strong, .stat-chip strong {
      display: block;
      margin-top: 3px;
      color: var(--ink);
      font-size: 11px;
      font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
      line-height: 1.35;
    }
    .legend-grid {
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: 8px;
    }
    .legend-chip {
      display: flex;
      gap: 8px;
      align-items: center;
      padding: 8px 10px;
      border: 1px solid rgba(255,255,255,0.08);
      border-radius: 12px;
      background: rgba(255,255,255,0.018);
    }
    .legend-chip__swatch {
      width: 12px;
      height: 12px;
      border-radius: 2px;
      background: var(--accent);
      box-shadow: 0 0 0 1px rgba(255,255,255,0.08);
    }
    .legend-chip__swatch--input { background: var(--input); }
    .legend-chip__swatch--signal { background: var(--signal); }
    .legend-chip__swatch--effect { background: var(--effect); }
    .legend-chip strong {
      font-size: 12px;
      line-height: 1.2;
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }
    .stat-band {
      display: grid;
      grid-template-columns: repeat(7, minmax(140px, 1fr));
      gap: 10px;
    }
    .canvas-surface {
      position: relative;
      width: calc(var(--canvas-width) * 1px - 52px);
      height: calc(var(--canvas-height) * 1px - 244px);
      margin-top: 18px;
    }
    .sidebar {
      position: absolute;
      display: grid;
      gap: 16px;
      align-content: start;
      z-index: 4;
    }
    .state-panel,
    .effect-panel,
    .invariant-panel {
      background: var(--panel);
      border: 1px solid rgba(255,255,255,0.08);
      border-radius: 14px;
      padding: 14px;
      box-shadow:
        inset 0 1px 0 rgba(255,255,255,0.03),
        0 10px 22px rgba(0,0,0,0.16);
      clip-path: polygon(10px 0, calc(100% - 10px) 0, 100% 10px, 100% calc(100% - 10px), calc(100% - 10px) 100%, 10px 100%, 0 calc(100% - 10px), 0 10px);
    }
    .state-panel__title {
      margin: 8px 0 0;
      font-size: 19px;
      letter-spacing: -0.035em;
      line-height: 1.05;
    }
    .state-field-list {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 6px;
      margin-top: 10px;
    }
    .state-field {
      min-height: 48px;
      padding: 8px 9px;
      border-radius: 10px;
      background: rgba(255,255,255,0.03);
      border: 1px solid rgba(255,255,255,0.06);
    }
    .state-field strong {
      display: block;
      font-size: 12px;
      line-height: 1.3;
    }
    .state-field span {
      display: block;
      margin-top: 4px;
      color: var(--muted);
      font-size: 10px;
      line-height: 1.4;
      font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    }
    .schematic-column {
      position: absolute;
      z-index: 3;
    }
    .schematic-plate {
      position: relative;
      background:
        radial-gradient(circle at 50% 0%, rgba(255,255,255,0.028), transparent 32%),
        linear-gradient(180deg, rgba(255,255,255,0.012), transparent 12%),
        var(--plate);
      border: 1px solid rgba(255,255,255,0.08);
      border-radius: 18px;
      box-shadow:
        inset 0 1px 0 rgba(255,255,255,0.04),
        inset 0 0 0 1px rgba(0,0,0,0.12),
        0 18px 42px rgba(0,0,0,0.22);
      overflow: hidden;
      clip-path: polygon(16px 0, calc(100% - 16px) 0, 100% 16px, 100% calc(100% - 16px), calc(100% - 16px) 100%, 16px 100%, 0 calc(100% - 16px), 0 16px);
    }
    .schematic-svg {
      position: absolute;
      inset: 0;
      width: 100%;
      height: 100%;
    }
    .phase-card,
    .cluster {
      position: absolute;
      overflow: hidden;
      border-radius: 14px;
      backdrop-filter: blur(3px);
      clip-path: polygon(10px 0, calc(100% - 10px) 0, 100% 10px, 100% calc(100% - 10px), calc(100% - 10px) 100%, 10px 100%, 0 calc(100% - 10px), 0 10px);
    }
    .phase-card {
      padding: 16px 18px 14px;
      border: 1px solid rgba(255,255,255,0.12);
      background: linear-gradient(180deg, rgba(109,143,160,0.11), rgba(255,255,255,0.03));
      box-shadow:
        0 14px 30px rgba(0,0,0,0.18),
        inset 0 1px 0 rgba(255,255,255,0.05);
    }
    .phase-card--emphasis {
      border-color: rgba(201,109,84,0.45);
      background: linear-gradient(180deg, rgba(201,109,84,0.16), rgba(255,255,255,0.04));
      box-shadow:
        0 24px 40px rgba(0,0,0,0.24),
        inset 0 1px 0 rgba(255,255,255,0.06);
    }
    .phase-card--sink {
      border-color: rgba(201,109,84,0.5);
      background: linear-gradient(180deg, rgba(68,18,18,0.8), rgba(25,12,12,0.95));
    }
    .phase-card h2 {
      margin: 8px 0 8px;
      font-size: 36px;
      line-height: 0.98;
      letter-spacing: -0.045em;
      font-weight: 600;
    }
    .phase-card__subtitle {
      color: var(--muted);
      font-size: 12px;
      line-height: 1.45;
    }
    .phase-card__stats {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin-top: 12px;
      font-size: 11px;
      color: var(--ink);
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }
    .phase-card__stats span {
      padding: 4px 8px;
      border-radius: 999px;
      background: rgba(255,255,255,0.08);
      border: 1px solid rgba(255,255,255,0.06);
    }
    .phase-card__stats--secondary span {
      color: var(--muted);
      background: rgba(255,255,255,0.04);
    }
    .cluster {
      padding: 14px 16px 14px;
      border: 1px solid rgba(255,255,255,0.08);
      background: linear-gradient(180deg, rgba(255,255,255,0.028), rgba(255,255,255,0.012));
      box-shadow: 0 16px 34px rgba(0,0,0,0.18);
    }
    .cluster--input {
      border-top: 3px solid var(--input);
    }
    .cluster--signal {
      border-top: 3px solid var(--signal);
    }
    .cluster--hybrid {
      border-top: 3px solid var(--accent);
    }
    .cluster__header {
      display: flex;
      justify-content: space-between;
      gap: 12px;
      align-items: flex-start;
    }
    .cluster__header h3 {
      margin: 8px 0 0;
      font-size: 20px;
      line-height: 1.05;
      letter-spacing: -0.035em;
      font-weight: 600;
    }
    .cluster__counts {
      display: grid;
      gap: 4px;
      color: var(--muted);
      font-size: 11px;
      text-transform: uppercase;
      letter-spacing: 0.12em;
      text-align: right;
    }
    .cluster__grid {
      display: grid;
      grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
      gap: 7px 10px;
      margin-top: 10px;
    }
    .trigger-row {
      display: grid;
      grid-template-columns: 10px minmax(0, 1fr);
      gap: 10px;
      align-items: start;
      padding: 6px 0 6px;
      border-top: 1px solid rgba(255,255,255,0.05);
    }
    .trigger-row:first-child,
    .cluster__grid > .trigger-row:nth-child(-n + var(--cols)) {
      border-top: none;
    }
    .trigger-row__kind {
      width: 10px;
      height: 10px;
      margin-top: 5px;
      border-radius: 2px;
      background: var(--accent);
    }
    .trigger-row--input .trigger-row__kind {
      background: var(--input);
    }
    .trigger-row--signal .trigger-row__kind {
      background: var(--signal);
    }
    .trigger-row__name {
      font-size: 12px;
      font-weight: 700;
      line-height: 1.25;
      letter-spacing: 0.01em;
    }
    .trigger-row__path {
      margin-top: 2px;
      color: var(--muted);
      font-size: 9px;
      line-height: 1.3;
      font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    }
    .cluster__footer {
      display: grid;
      gap: 8px;
      margin-top: 10px;
      padding-top: 10px;
      border-top: 1px solid rgba(255,255,255,0.06);
      opacity: 0.72;
    }
    .token-lane {
      display: grid;
      grid-template-columns: 66px 1fr;
      gap: 10px;
      align-items: start;
    }
    .token-lane__label {
      color: var(--faint);
      text-transform: uppercase;
      letter-spacing: 0.14em;
      font-size: 10px;
      padding-top: 4px;
    }
    .token-lane__tokens,
    .effect-chip-list {
      display: flex;
      flex-wrap: wrap;
      gap: 6px;
    }
    .token-chip,
    .effect-chip {
      display: inline-flex;
      align-items: center;
      padding: 4px 8px;
      border-radius: 999px;
      font-size: 10px;
      line-height: 1.3;
      font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
      border: 1px solid rgba(255,255,255,0.08);
      background: rgba(255,255,255,0.04);
      color: var(--ink);
    }
    .token-chip--overflow {
      color: var(--muted);
    }
    .bottom-band {
      display: none;
    }
    .effect-lanes {
      display: grid;
      gap: 10px;
      margin-top: 14px;
    }
    .effect-lane {
      padding: 12px 12px 14px;
      border-radius: 12px;
      border: 1px solid rgba(255,255,255,0.07);
      background: rgba(255,255,255,0.03);
    }
    .effect-lane__title {
      margin-bottom: 10px;
      font-size: 12px;
      letter-spacing: 0.15em;
      text-transform: uppercase;
      color: var(--faint);
    }
    .effect-lane--routed { border-left: 4px solid var(--routed); }
    .effect-lane--external { border-left: 4px solid var(--external); }
    .effect-lane--local { border-left: 4px solid var(--local); }
    .invariant-grid {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 10px;
      margin-top: 14px;
    }
    .invariant-card {
      padding: 12px 14px;
      border-radius: 12px;
      border: 1px solid rgba(255,255,255,0.07);
      background: rgba(255,255,255,0.03);
    }
    .invariant-card h3 {
      margin: 0 0 8px;
      font-size: 15px;
      letter-spacing: -0.02em;
    }
    .invariant-card pre {
      margin: 0;
      color: var(--muted);
      font-size: 11px;
      line-height: 1.5;
      white-space: pre-wrap;
      word-break: break-word;
    }
    .footer-note {
      display: flex;
      justify-content: space-between;
      gap: 16px;
      margin-top: 16px;
      padding: 14px 4px 0;
      border-top: 1px solid rgba(255,255,255,0.08);
      color: var(--faint);
      font-size: 12px;
      line-height: 1.5;
    }
    .footer-note--plate {
      margin-top: 10px;
    }
    .metrics-rack {
      position: absolute;
      top: 0;
      left: 0;
      display: grid;
      grid-template-columns: repeat(7, minmax(72px, 1fr));
      gap: 8px;
      width: 600px;
      z-index: 5;
    }
    .effect-panel--canvas,
    .invariant-panel--canvas {
      position: absolute;
      z-index: 4;
    }
    @media (max-width: 2200px) {
      .meta-grid {
        grid-template-columns: 1fr 1fr;
        min-width: 480px;
      }
    }
    @media (max-width: 1680px) {
      .plate-header {
        flex-direction: column;
      }
      .legend-plaque {
        width: 100%;
      }
      .meta-grid {
        width: 100%;
      }
      .canvas-surface {
        overflow-x: auto;
        overflow-y: hidden;
      }
    }`;
}

function renderTriggerPath(transitions) {
  const segments = transitions.map((transition) => {
    const from = transition.from.join(" · ");
    if (transition.from.length === 1 && transition.from[0] === transition.to) {
      return `${from} ⟲`;
    }
    return `${from} → ${transition.to}`;
  });
  return segments.join(" / ");
}

function normalizePhaseList(label, phaseOrder) {
  const order = new Map(phaseOrder.map((phase, index) => [phase, index]));
  return label
    .split(" / ")
    .map((segment) => {
      if (!segment.includes("→") && !segment.includes("⟲")) {
        return segment;
      }
      const arrow = segment.includes("⟲") ? "⟲" : "→";
      const parts = segment.split(arrow);
      const left = parts[0]
        .split(" · ")
        .map((item) => item.trim())
        .sort((a, b) => (order.get(a) ?? 999) - (order.get(b) ?? 999))
        .join(" · ");
      const right = parts[1] ? parts[1].trim() : "";
      return arrow === "⟲" ? `${left} ⟲` : `${left} → ${right}`;
    })
    .join(" / ");
}

function extractWrites(body) {
  return unique(
    [...body.matchAll(/\b([A-Za-z_][A-Za-z0-9_]*)'\s*=/g)].map((match) => match[1]),
  );
}

function parseSignatureBullet(line) {
  const match = line.match(/^- `([^`]+)`(?:\((.*)\))?/);
  if (!match) return null;
  return {
    name: match[1],
    signature: `${match[1]}(${match[2] ?? ""})`,
  };
}

function extractBackticked(line) {
  return [...line.matchAll(/`([^`]+)`/g)].map((match) => match[1]);
}

function unique(items) {
  return [...new Set(items)];
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function cleanGeneratedText(text) {
  return text.replace(/[ \t]+$/gm, "");
}
