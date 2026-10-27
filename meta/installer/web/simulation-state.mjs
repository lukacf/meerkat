import { identities, resolvedEdges } from "./state.mjs";
import { workflowLayers } from "./wizard-state.mjs";
import { simulationDesignContext, simulationVisualizationFeedback } from "./simulation-design-context.mjs";

const INPUT_ID = "boundary:console:input";
const OUTPUT_ID = "boundary:console:output";
const copy = (value) => structuredClone(value);
const unique = (values) => [...new Set(values)];

const TRIAL_ACTIVITY_STATUS = Object.freeze({
  queued: "queued", starting: "starting", running: "working",
  waiting_input: "waiting for your follow-up", ending: "ending", canceling: "stopping",
  completed: "finished", failed: "stopped with an error", interrupted: "interrupted",
  unavailable: "unavailable", replaying: "replaying saved activity",
});

/** Display wording only; retain the actual trial status in its receipt. */
export function trialActivityStatus(status) {
  return typeof status === "string" && Object.hasOwn(TRIAL_ACTIVITY_STATUS, status)
    ? TRIAL_ACTIVITY_STATUS[status] : "saved activity";
}

const edgeId = (kind, ...parts) =>
  `${kind}:${parts.map((part) => encodeURIComponent(part)).join("|")}`;

const LOCAL_RESOURCES = {
  workspace: {
    id: "resource:local:workspace",
    name: "Workspace files",
    kind: "workspace",
    role: "Local text sources",
    instructions:
      "Assigned agents can list and read UTF-8 files in this deployment's workspace. This grant does not convert or ingest documents.",
    label: "Read workspace text",
  },
  notes: {
    id: "resource:local:notes",
    name: "Shared notes",
    kind: "notes",
    role: "Local team knowledge",
    instructions:
      "Local SQLite notes shared by agents with a notes grant. Each edge identifies read or write access. These are separate from each identity's conversation memory.",
  },
  builtins: {
    id: "resource:tool:builtins",
    name: "Built-in tools",
    kind: "tool",
    role: "Native tool access",
    instructions:
      "This class has the blueprint's native built-in tool grant. Available tools are determined by the runtime.",
    label: "Built-in tool access",
  },
  shell: {
    id: "resource:tool:shell",
    name: "Local commands",
    kind: "tool",
    role: "Shell tool access",
    instructions:
      "Assigned agents have the native shell tool grant. The preview does not execute commands or claim any command has run.",
    label: "Shell access",
  },
  workgraph: {
    id: "resource:tool:workgraph",
    name: "Task tracking",
    kind: "tool",
    role: "Local work state",
    instructions:
      "Assigned agents have WorkGraph tools for tracking work. This is distinct from changing records in an external service.",
    label: "WorkGraph access",
  },
  schedule: {
    id: "resource:tool:schedule",
    name: "Scheduling tools",
    kind: "tool",
    role: "Native scheduling capability",
    instructions:
      "Assigned agents have scheduling tools. A tool grant does not install a recurring trigger; this blueprint does not configure an automatic schedule.",
    label: "Scheduling tool access",
  },
};

function phase(
  id,
  title,
  description,
  kind,
  nodeIds,
  edgeIds = [],
  stepIds = [],
) {
  return {
    id,
    title,
    description,
    kind,
    activeNodeIds: unique(nodeIds),
    activeEdgeIds: unique(edgeIds),
    stepIds: [...stepIds],
  };
}

/** Guide-confirmed design context, never an implemented route or access grant. */
export function simulationInterfaceContext(fact) {
  const confirmed = fact?.status === "confirmed" && typeof fact.summary === "string"
    && fact.summary.trim().length > 0 && [...fact.summary].length <= 1600
    && Array.isArray(fact.source_message_ids) && fact.source_message_ids.length > 0
    && fact.source_message_ids.length <= 40
    && fact.source_message_ids.every(id => typeof id === "string" && id.trim().length > 0)
    && new Set(fact.source_message_ids).size === fact.source_message_ids.length;
  return {
    status: confirmed ? "confirmed" : "unresolved",
    summary: confirmed ? fact.summary : null,
    source_message_ids: confirmed ? [...fact.source_message_ids] : [],
    interpretation: "These are human interface design requirements, not verified UI, authentication or routing. Keep everyday user interfaces distinct from runtime/operator Console when the requirements distinguish them. The native topology entrypoint is its configured default identity, not a human contact mapping. Legacy boundary IDs do not select an interface. Unconnected human boundaries mean no contact or delivery edge is configured here; they do not establish that intended ownership is unknown. Use the current design_context and agent instructions to illustrate an explicitly intended conversation and result handoff, labelled as intended design. Leave ownership open when that source does not identify it. Neither the native default, external addressability nor peer links establish human routing. Such an illustration creates no configured contact edge or grant.",
  };
}

/** The complete effective inputs own generation, drafts and delayed responses. */
export function simulationSourceKey(plan, model) {
  return JSON.stringify([plan, model]);
}

/**
 * A deterministic explanatory preview of a proposed deployment.
 * Edges express configuration, never observed runtime activity. Only native DAG
 * dependency edges establish order; peer and tool edges express permissions.
 */
export function createSimulation(plan, flowId, interfaceFact, designBrief, visualizationFeedback) {
  const empty = {
    nodes: [],
    edges: [],
    phases: [],
    flows: [],
    selectedFlowId: null,
    entrypoint: null,
  };
  if (!plan?.agent_classes?.length) return empty;

  const interfaceContext = simulationInterfaceContext(interfaceFact);
  const designContext = simulationDesignContext(designBrief);
  const compositionFeedback = simulationVisualizationFeedback(visualizationFeedback);

  const classes = new Map(plan.agent_classes.map((item) => [item.id, item]));
  const members = identities(plan);
  if (!members.length) return empty;
  const profileOnlyClasses = plan.agent_classes.filter((item) => item.instances === 0).map(copy);
  const memberIds = new Set(members.map((item) => item.id));
  const entrypoint = memberIds.has(plan.topology?.entrypoint)
    ? plan.topology.entrypoint
    : members[0].id;
  const names = new Map(members.map((item) => [item.id, item.name]));
  const memberIndexes = new Map();
  const nodes = members.map((member) => {
    const agentClass = classes.get(member.classId);
    const memberIndex = memberIndexes.get(member.classId) || 0;
    memberIndexes.set(member.classId, memberIndex + 1);
    const instructions = [
      agentClass.instructions,
      agentClass.members?.[memberIndex]?.instructions,
    ]
      .filter(Boolean)
      .join("\n\n");
    return {
      ...member,
      kind: "agent",
      instructions,
      tools: copy(agentClass.tools || {}),
    };
  });
  const edges = [];
  const phases = [];
  const nodeIds = new Set(memberIds);
  const edgeIds = new Set();
  function addNode(node) {
    if (nodeIds.has(node.id)) return;
    nodeIds.add(node.id);
    nodes.push({ classId: null, ...node });
  }
  function addEdge(edge) {
    if (
      edgeIds.has(edge.id) ||
      !nodeIds.has(edge.source) ||
      !nodeIds.has(edge.target)
    )
      return;
    edgeIds.add(edge.id);
    edges.push(edge);
  }

  const flows = (plan.flows || []).map((flow) => {
    const steps = copy(flow.steps || []);
    const layers = workflowLayers({ steps });
    return {
      id: flow.id,
      name: flow.name,
      description: flow.description || "",
      stepCount: steps.length,
      layerCount: layers.length,
      valid:
        steps.length > 0 &&
        layers.length > 0 &&
        steps.every((step) => memberIds.has(step.agent)),
      steps,
    };
  });
  const selected = flowId === '__overview__' ? null : flows.find((flow) => flow.id === flowId) || flows[0] || null;
  addNode({
    id: INPUT_ID,
    name: "You",
    kind: "input",
    role: "Human interface",
    instructions: `Use interface_context, current design_context and agent instructions for the intended human interaction. No human contact edge is configured in this model. When that source identifies a conversation owner, illustrate the intended interaction with that owner and label it as intended design; otherwise leave ownership open. ${names.get(entrypoint)} is the configured native default, not a human contact mapping. This unconnected boundary grants no access.${selected ? ` The ${selected.name} workflow is launched explicitly; it is not triggered by a communication link.` : " Peer links permit collaboration; they do not start work automatically."}`,
  });
  phases.push(
    phase(
      "conversation-entry",
      "How you use the application",
      interfaceContext.status === "confirmed"
        ? "The confirmed interface requirements and current owner instructions explain the intended human interaction. Show a source-backed intended conversation separately from configured native routing."
        : "The human interface is not confirmed. The current brief and owner instructions may still identify an intended conversation; leave ownership open when they do not. The native default does not establish a human route.",
      "input",
      [INPUT_ID],
    ),
  );

  const topology = {
    ...plan,
    topology: { mode: "manual", edges: [], ...plan.topology, entrypoint },
  };
  for (const link of resolvedEdges(topology) || []) {
    if (link.a === link.b) continue;
    addEdge({
      id: edgeId("peer", ...[link.a, link.b].sort()),
      source: link.a,
      target: link.b,
      kind: "peer",
      directed: false,
      label: "Can exchange peer messages",
    });
  }

  const connections = new Map(
    (plan.connections || []).map((connection) => [connection.id, connection]),
  );
  function grant(member, node, label, extra = {}) {
    addNode(node);
    addEdge({
      id: edgeId("tool", member.id, node.id),
      source: member.id,
      target: node.id,
      kind: "tool",
      label,
      ...extra,
    });
  }
  for (const member of members) {
    const tools = classes.get(member.classId).tools || {};
    for (const key of Object.keys(LOCAL_RESOURCES)) {
      if (!tools[key] || tools[key] === "none") continue;
      const resource = LOCAL_RESOURCES[key];
      const { label, ...node } = resource;
      grant(
        member,
        node,
        key === "notes"
          ? tools.notes === "write"
            ? "Read and write notes"
            : "Read notes"
          : label,
        {
          capability: key,
          access: key === "notes" ? tools.notes : "configured",
        },
      );
    }
    if (tools.knowledge && ["local", "remote"].includes(plan.knowledge?.mode)) {
      grant(
        member,
        {
          id: "resource:knowledge:elephant",
          name: "Knowledge service",
          kind: "knowledge",
          role:
            plan.knowledge.mode === "local"
              ? "Local knowledge service"
              : "Existing knowledge service",
          instructions:
            "Read-only access to the configured knowledge space. Content needs separate operator-led ingestion or an implemented ingestion pipeline. This preview does not ingest data or verify the service is ready.",
        },
        "Read configured knowledge space",
        { capability: "knowledge", access: "read" },
      );
    }
    for (const access of tools.mcp || []) {
      const connection = connections.get(access.connection_id);
      if (!connection) continue;
      const toolNames = [...(access.tool_names || [])];
      grant(
        member,
        {
          id: `resource:connector:${connection.id}`,
          name: connection.name,
          kind: "connector",
          role: "Configured MCP connection",
          instructions:
            connection.description ||
            "A configured tool connection. Credentials and a successful handshake are required before use.",
          connectionId: connection.id,
          transport: connection.transport,
        },
        toolNames.length === 1
          ? toolNames[0]
          : toolNames.length > 1
            ? `${toolNames.length} selected tools`
            : "All advertised tools",
        { capability: "mcp", connectionId: connection.id, toolNames },
      );
    }
  }

  function accessPhase(id, agentIds, stepIds = []) {
    const matching = edges.filter(
      (edge) => edge.kind === "tool" && agentIds.includes(edge.source),
    );
    if (!matching.length) return;
    phases.push(
      phase(
        id,
        "Configured tools and sources",
        "These agents can use the highlighted tools and sources. Select a source to see who has access and which tools are available.",
        "access",
        matching.flatMap((edge) => [edge.source, edge.target]),
        matching.map((edge) => edge.id),
        stepIds,
      ),
    );
  }

  if (selected?.valid) {
    const workflowId = `boundary:workflow:${selected.id}`;
    addNode({
      id: workflowId,
      name: 'Workflow runner',
      kind: 'workflow',
      role: 'Native flow dispatch',
      instructions: `An explicit workflow invocation starts ${selected.name}. The native flow runner dispatches its root steps and supplies their inputs. This is separate from any intended human interaction; the blueprint does not configure a human contact path or automatic trigger.`,
    });
    const byStep = new Map(selected.steps.map((step) => [step.id, step]));
    for (const step of selected.steps) {
      if (!(step.depends_on || []).length) {
        addEdge({
          id: edgeId("activation", selected.id, step.id),
          source: workflowId,
          target: step.agent,
          kind: "activation",
          label: `Native workflow dispatch to ${step.id}`,
          targetStepId: step.id,
        });
      }
      for (const dependency of step.depends_on || []) {
        const source = byStep.get(dependency);
        addEdge({
          id: edgeId("flow", selected.id, dependency, step.id),
          source: source.agent,
          target: step.agent,
          kind: "flow",
          label: `Output from ${dependency} to ${step.id}`,
          sourceStepId: dependency,
          targetStepId: step.id,
        });
      }
    }
    for (const [index, layer] of workflowLayers(selected).entries()) {
      const stepIds = layer.map((step) => step.id);
      const agentIds = unique(layer.map((step) => step.agent));
      const incoming = edges.filter(
        (edge) =>
          ["activation", "flow"].includes(edge.kind) &&
          stepIds.includes(edge.targetStepId),
      );
      phases.push(
        phase(
          `flow:${selected.id}:inputs:${index}`,
          index === 0
            ? `Launch ${selected.name}`
            : "Pass the required evidence",
          index === 0
            ? "An explicit workflow run dispatches the initial tasks. These workflow inputs do not establish a human contact owner."
            : "Each next step receives the outputs it depends on. Follow the moving messages to see which evidence reaches each agent.",
          "handoff",
          incoming.flatMap((edge) => [edge.source, edge.target]),
          incoming.map((edge) => edge.id),
          stepIds,
        ),
      );
      accessPhase(`flow:${selected.id}:access:${index}`, agentIds, stepIds);
      phases.push(
        phase(
          `flow:${selected.id}:work:${index}`,
          layer.length > 1
            ? `${layer.length} steps in parallel`
            : `${names.get(layer[0].agent)} works`,
          layer
            .map((step) => `${names.get(step.agent)}: ${step.prompt}`)
            .join("\n\n"),
          "work",
          agentIds,
          [],
          stepIds,
        ),
      );
    }
    const dependedOn = new Set(
      selected.steps.flatMap((step) => step.depends_on || []),
    );
    const terminal = selected.steps.filter((step) => !dependedOn.has(step.id));
    addNode({
      id: OUTPUT_ID,
      name: "Workflow result",
      kind: "output",
      role: "Native workflow output",
      instructions:
        "The configured workflow returns its terminal step outputs for inspection. This is a preview of the design, not a completed run or an external delivery.",
    });
    const resultEdges = terminal.map((step) => ({
      id: edgeId("output", selected.id, step.id),
      source: step.agent,
      target: OUTPUT_ID,
      kind: "output",
      label: `Result from ${step.id}`,
      sourceStepId: step.id,
    }));
    resultEdges.forEach(addEdge);
    phases.push(
      phase(
        `flow:${selected.id}:result`,
        "Collect the workflow result",
        "The final step outputs are available for inspection. The intended human interface follows interface_context; no real delivery has run in this preview.",
        "output",
        [...terminal.map((step) => step.agent), OUTPUT_ID],
        resultEdges.map((edge) => edge.id),
        terminal.map((step) => step.id),
      ),
    );
  } else if (selected) {
    phases.push(
      phase(
        `flow:${selected.id}:invalid`,
        "This workflow needs a correction",
        "A missing identity, dependency or dependency cycle prevents this workflow from being previewed. The proposal needs a valid native DAG.",
        "blocked",
        unique(
          selected.steps
            .map((step) => step.agent)
            .filter((id) => memberIds.has(id)),
        ),
      ),
    );
  } else {
    for (const item of plan.agent_classes) {
      accessPhase(
        `conversation:access:${item.id}`,
        members
          .filter((member) => member.classId === item.id)
          .map((member) => member.id),
      );
    }
    const peers = edges.filter((edge) => edge.kind === "peer");
    if (peers.length) {
      phases.push(
        phase(
          "conversation:peer-permissions",
          "Who can collaborate",
          "These agents can exchange messages when they need one another. Select an agent to explore its collaborators and responsibilities.",
          "permissions",
          peers.flatMap((edge) => [edge.source, edge.target]),
          peers.map((edge) => edge.id),
        ),
      );
    }
    phases.push(phase("conversation:native-default", "Configured native default", `${names.get(entrypoint)} is the configured default identity for the native topology. That default does not establish who receives a person's request.`, "configuration", [entrypoint]));
    addNode({
      id: OUTPUT_ID,
      name: "Result for you",
      kind: "output",
      role: "Human result boundary",
      instructions: "Use interface_context, current design_context and agent instructions to explain the intended result and its producing owner when the source identifies one. Label any illustrated human handoff as intended design; no delivery edge is configured here. This unconnected boundary does not verify delivery, authentication or approval. Choose a workflow above to inspect its known native output path.",
    });
    phases.push(
      phase(
        "conversation:response",
        "A result for you",
        "Show the intended result handoff when the current brief or owner instructions identify it, and leave unspecified ownership open. This illustrates the design; no real work or delivery has run in this preview.",
        "output",
        [OUTPUT_ID],
      ),
    );
  }

  return {
    nodes,
    edges,
    phases,
    flows,
    selectedFlowId: selected?.id || null,
    entrypoint,
    interface_context: interfaceContext,
    ...(profileOnlyClasses.length ? {profile_only_classes: profileOnlyClasses} : {}),
    ...(designContext ? {design_context: designContext} : {}),
    ...(compositionFeedback ? {visualization_feedback: compositionFeedback} : {}),
  };
}
