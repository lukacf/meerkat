import { identities, resolvedEdges } from "./state.mjs";

export function guidePlan(state) {
  return guideStagedDesign(state)?.blueprint || state?.proposal || state?.blueprint || null;
}

/** A checked pair or explicitly context-pending topology belongs to one active turn. */
export function guideStagedDesign(state) {
  const staged = state?.live_design, operation = state?.operation;
  const paired = staged?.context_pending === undefined && staged?.brief &&
    typeof staged.brief === "object" && !Array.isArray(staged.brief);
  const awaitingContext = staged?.context_pending === true && staged.brief === null;
  return operation?.status === "running" && Number.isSafeInteger(state.version) && state.version > 0 &&
    operation.version === state.version && staged?.guide_version === state.version &&
    typeof state.id === "string" && !!state.id && staged.guide_id === state.id &&
    typeof operation.request_id === "string" && /^[a-f0-9]{64}$/.test(operation.request_id) &&
    staged.request_id === operation.request_id && staged.blueprint && typeof staged.blueprint === "object" &&
    !Array.isArray(staged.blueprint) && (paired || awaitingContext)
    ? staged : null;
}

export function guideDesignBrief(state, visiblePlan = guidePlan(state)) {
  const staged = guideStagedDesign(state);
  return staged?.blueprint === visiblePlan ? staged.brief : state?.brief;
}

export function guideVisualizationFeedback(state, visiblePlan = guidePlan(state)) {
  const staged = guideStagedDesign(state);
  // An unpaired topology has no current checked presentation preferences yet.
  return staged?.blueprint === visiblePlan
    ? staged.context_pending === true ? null : staged.visualization_feedback
    : state?.visualization_feedback;
}

export function hasDecision(state, stage) {
  return (
    state?.status === "ready" ||
    state?.completed_stages?.includes(stage) ||
    state?.answers?.some((answer) => answer.stage === stage) ||
    false
  );
}

export function toolSummary(tools = {}) {
  const items = [];
  if (tools.workspace) items.push("Workspace text files");
  if (tools.notes && tools.notes !== "none")
    items.push(tools.notes === "write" ? "Shared notes" : "Read shared notes");
  if (tools.memory) items.push("Conversation memory");
  if (tools.knowledge) items.push("Knowledge service");
  if (tools.builtins) items.push("Built-in tools");
  if (tools.shell) items.push("Shell access");
  if (tools.workgraph) items.push("Shared work graph");
  if (tools.schedule) items.push("Scheduling");
  const grants = tools.mcp || [];
  if (grants.length)
    items.push(
      `${grants.length} app ${grants.length === 1 ? "connection" : "connections"}`,
    );
  const summary = tools.read_only === true
    ? ["Native read-only policy (declared)", ...items.map((item) => `Grant: ${item}`)]
    : items;
  const bundleCount = tools.rust_bundles?.length || 0;
  if (bundleCount)
    summary.push(`${bundleCount} native ${bundleCount === 1 ? "bundle" : "bundles"} selected`);
  return summary;
}

export function connectionDetails(connection, plan) {
  const users = (plan?.agent_classes || []).filter((agent) =>
    agent.tools?.mcp?.some((grant) => grant.connection_id === connection.id),
  );
  const references = new Set(connection.required_environment || []);
  const values = [
    ...Object.values(connection.env || {}),
    ...Object.values(connection.headers || {}),
    connection.url || "",
    ...(connection.args || []),
  ];
  for (const value of values) {
    for (const match of String(value).matchAll(/\$\{([A-Z][A-Z0-9_]*)\}/g))
      references.add(match[1]);
  }
  return {
    users,
    environment: [...references],
    status: users.length ? "Setup at install" : "Not assigned",
    source:
      connection.source?.registry_id ||
      connection.source?.repository ||
      connection.source?.url ||
      connection.source?.version ||
      "Custom connection",
  };
}

export function setupFacts(state, plan = guidePlan(state)) {
  if (!plan) return [];
  const tools = plan.agent_classes.map((agent) => agent.tools || {});
  const knowledge = plan.knowledge?.mode;
  const memory = tools.some((tool) => tool.memory);
  const notes = tools.some((tool) => tool.notes && tool.notes !== "none");
  const knowledgeStatus = state?.brief?.facts?.knowledge?.status;
  // Canonical brief facts can record a choice before a legacy stage answer.
  // A proposed choice stays pending, and an unresolved fact supersedes old answers.
  const knowledgeDecided = knowledgeStatus
    ? knowledgeStatus === "confirmed"
    : hasDecision(state, "knowledge");
  const knowledgeValue =
    knowledge === "local"
      ? "New knowledge service"
      : knowledge === "remote"
        ? "Existing knowledge service"
        : [notes && "Shared notes", memory && "Agent memory"]
            .filter(Boolean)
            .join(" + ") || "No shared knowledge service";
  const autonomyAnswer = [...(state?.answers || [])]
    .reverse()
    .find((answer) => answer.stage === "autonomy");
  const chosenLabels = (autonomyAnswer?.question?.options || [])
    .filter((option) => autonomyAnswer.selected?.includes(option.id))
    .map((option) => option.label);
  const target =
    {
      mac: "On your Mac",
      linux: "On a Linux computer",
      gcp: "On Google Cloud",
    }[plan.target] || plan.target;
  const destinationFact = state?.brief?.facts?.destination;
  const destinationDecided = destinationFact
    ? destinationFact.status === "confirmed"
    : hasDecision(state, "destination");
  const authorityFact = state?.brief?.facts?.authority;
  const authorityDecided = authorityFact
    ? authorityFact.status === "confirmed"
    : hasDecision(state, "autonomy");
  const authorityValue = authorityFact?.summary || chosenLabels.join(", ") || "Custom tool permissions";
  const facts = [
    {
      label: "Runs on",
      value: destinationDecided
        ? target
        : destinationFact?.status === "proposed"
          ? `Proposed: ${target}`
        : "We'll decide together",
      pending: !destinationDecided,
    },
    {
      label: "Knowledge",
      value: knowledgeDecided
        ? knowledgeValue
        : knowledgeStatus === "proposed"
          ? `Proposed: ${knowledgeValue}`
          : "Not decided yet",
      pending: !knowledgeDecided,
    },
    {
      label: "Your preference",
      value: authorityDecided ? authorityValue
        : authorityFact?.status === "proposed" ? `Proposed: ${authorityValue}`
          : "Not decided yet",
      pending: !authorityDecided,
    },
    {
      label: "Current access",
      value:
        [
          tools.some((tool) => tool.shell) && "Shell",
          tools.some((tool) => tool.builtins) && "Built-in tools",
          tools.some((tool) => tool.notes === "write") && "Write shared notes",
          tools.some((tool) => tool.mcp?.length) && "App tools (see grants)",
          tools.some((tool) => tool.knowledge) && "Knowledge tools",
          tools.some((tool) => tool.workgraph) && "Work tracking",
          tools.some((tool) => tool.schedule) && "Scheduling",
          tools.some((tool) => tool.rust_bundles?.length) && "Native bundles selected (setup required)",
        ]
          .filter(Boolean)
          .join(", ") || "Conversation and read access",
    },
  ];
  if (
    destinationDecided &&
    plan.target === "gcp" &&
    plan.gcp?.project
  )
    facts.push({ label: "Project", value: plan.gcp.project });
  return facts;
}

export function classCountRequest(agent, count) {
  if (count === 0) return `Keep ${agent.name} (${agent.id}) as a reusable class with zero initial identities and empty members, retaining its responsibilities and grants. Do not replace real owners with a template or unassigned identity. Preserve an actual native entrypoint and valid fixed workflow targets; explain any conflict before removing a referenced owner.`;
  return `Please change the ${agent.name} class (${agent.id}) from ${agent.instances} to ${count} ${count === 1 ? "agent" : "agents"}. Update the plan's identities, topology and workflows to match, and keep my other choices.`;
}

function edgeKey(a, b) {
  return [a, b].sort().join("\0");
}

// The diagram groups actual identities by class. All links come from configured
// topology or grants; the animation is illustrative, never runtime telemetry.
export function planGraph(plan) {
  if (!plan)
    return {
      nodes: [],
      edges: [],
      height: 380,
      caption: "Your team will take shape here.",
    };
  const classes = plan.agent_classes;
  const identityClasses = new Map(
    identities(plan).map((identity) => [identity.id, identity.classId]),
  );
  const entryClass = identityClasses.get(plan.topology.entrypoint);
  const ordered = [...classes].sort(
    (a, b) => Number(b.id === entryClass) - Number(a.id === entryClass),
  );
  const apps = (plan.connections || []).filter((connection) =>
    classes.some((agent) =>
      agent.tools?.mcp?.some((grant) => grant.connection_id === connection.id),
    ),
  );
  const hasKnowledge =
    plan.knowledge?.mode !== "none" &&
    classes.some((agent) => agent.tools?.knowledge);
  const externalCount = apps.length + Number(hasKnowledge);
  const compact = classes.length <= 8;
  const rows = Math.ceil(classes.length / 4);
  const graphHeight = compact ? 350 : Math.max(360, rows * 104 + 70);
  const extraRows = externalCount ? Math.ceil(externalCount / 5) : 0;
  const height = graphHeight + (extraRows ? extraRows * 82 + 20 : 0);
  const nodes = ordered.map((agent, index) => {
    let x, y;
    if (ordered.length === 1) {
      x = 380;
      y = 185;
    } else if (ordered.length === 2) {
      x = 240 + index * 280;
      y = 185;
    } else if (compact) {
      const angle = -Math.PI / 2 + (index * 2 * Math.PI) / ordered.length;
      x = 380 + 235 * Math.cos(angle);
      y = 180 + 112 * Math.sin(angle);
    } else {
      const row = Math.floor(index / 4),
        count = Math.min(4, ordered.length - row * 4);
      x = (760 - count * 166) / 2 + 83 + (index % 4) * 166;
      y = 78 + row * 104;
    }
    return {
      id: `agent:${agent.id}`,
      classId: agent.id,
      label: agent.name,
      count: agent.instances,
      subtitle: agent.id,
      kind: agent.instances === 0 ? "profile" : "agent",
      entrypoint: agent.id === entryClass,
      x,
      y,
    };
  });
  const externals = apps.map((connection) => ({
    id: `connection:${connection.id}`,
    label: connection.name,
    kind: "connection",
    subtitle: "App connection",
  }));
  if (hasKnowledge)
    externals.push({
      id: "knowledge:elephant",
      label: "Knowledge service",
      kind: "knowledge",
      subtitle: "Knowledge",
    });
  externals.forEach((node, index) => {
    const row = Math.floor(index / 5),
      count = Math.min(5, externals.length - row * 5);
    nodes.push({
      ...node,
      x: (760 - count * 142) / 2 + 71 + (index % 5) * 142,
      y: graphHeight + 37 + row * 82,
    });
  });
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const seen = new Set(),
    edges = [];
  const add = (a, b, kind) => {
    if (!byId.has(a) || !byId.has(b) || a === b || seen.has(edgeKey(a, b)))
      return;
    seen.add(edgeKey(a, b));
    const source = byId.get(a),
      target = byId.get(b);
    edges.push({
      id: `${a}__${b}`,
      a,
      b,
      kind,
      path: `M ${source.x} ${source.y} L ${target.x} ${target.y}`,
      caption: `${source.label} ${kind === "communication" ? "can coordinate with" : "has access to"} ${target.label}.`,
    });
  };
  for (const edge of resolvedEdges(plan))
    add(
      `agent:${identityClasses.get(edge.a)}`,
      `agent:${identityClasses.get(edge.b)}`,
      "communication",
    );
  for (const agent of classes) {
    for (const grant of agent.tools?.mcp || [])
      add(
        `agent:${agent.id}`,
        `connection:${grant.connection_id}`,
        "connection",
      );
    if (agent.tools?.knowledge && hasKnowledge)
      add(`agent:${agent.id}`, "knowledge:elephant", "knowledge");
  }
  return {
    nodes,
    edges,
    height,
    caption: edges.length
      ? edges[0].caption
      : `${classes[0].name} is your starting point.`,
    grouped: classes.some((agent) => agent.instances > 1),
  };
}
