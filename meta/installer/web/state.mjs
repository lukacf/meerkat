import limits from "../launchkit/blueprint-limits.json" with { type: "json" };

export const PLANNING_LIMITS = Object.freeze(limits);
export const DEFAULT_MODELS = {
  openai: "gpt-6.1-sol",
  anthropic: "claude-sonnet-4-6",
  gemini: "gemini-3.1-pro-preview",
};
export const DRAFT_KEY = "mobkit-launch-blueprint-v2";
export function clone(value) {
  return structuredClone(value);
}
export function slugify(value) {
  const safe = value
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 36);
  return /^[a-z]/.test(safe) ? safe || "my-mob" : `mob-${safe || "project"}`;
}
export function identities(blueprint) {
  return blueprint.agent_classes.flatMap((c) =>
    Array.from({ length: c.instances }, (_, i) => ({
      id: `${c.id}:${c.members?.[i]?.key || (c.instances === 1 ? "main" : i + 1)}`,
      classId: c.id,
      name: c.members?.[i]?.name || (c.instances === 1 ? c.name : `${c.name} ${i + 1}`),
      role: c.description,
    })),
  );
}
export function resolvedEdges(blueprint) {
  const ids = identities(blueprint).map((a) => a.id);
  const entry = ids.includes(blueprint.topology.entrypoint)
    ? blueprint.topology.entrypoint
    : ids[0];
  switch (blueprint.topology.mode) {
    case "hub":
      return ids
        .filter((id) => id !== entry)
        .map((id) => ({ a: entry, b: id }));
    case "mesh":
      return ids.flatMap((a, i) => ids.slice(i + 1).map((b) => ({ a, b })));
    case "pipeline":
      return ids.slice(0, -1).map((a, i) => ({ a, b: ids[i + 1] }));
    default:
      return blueprint.topology.edges;
  }
}
export function uniqueId(base, existing) {
  const ids = new Set(existing);
  let value = base,
    counter = 2;
  while (ids.has(value)) value = `${base.slice(0, 33)}-${counter++}`;
  return value;
}
export function newClass(blueprint) {
  return {
    id: uniqueId(
      "specialist",
      blueprint.agent_classes.map((c) => c.id),
    ),
    name: "New specialist",
    description: "A focused specialist with a clear responsibility.",
    instructions:
      "Help the team with your area of expertise. Explain your evidence, ask for missing context, and coordinate with your peers.",
    provider: blueprint.provider,
    model: blueprint.model,
    instances: 1,
    tools: {
      workspace: false,
      notes: "none",
      builtins: false,
      knowledge: false,
      mcp: [],
    },
  };
}
export function classInstanceLimit(blueprint, classId) {
  const others = blueprint.agent_classes.reduce(
    (total, agent) => total + (agent.id === classId ? 0 : agent.instances), 0,
  );
  const populationLimit = blueprint.topology.mode === "mesh"
    ? Math.min(PLANNING_LIMITS.maxIdentities, Math.floor((1 + Math.sqrt(1 + 8 * PLANNING_LIMITS.maxPeerEdges)) / 2))
    : PLANNING_LIMITS.maxIdentities;
  return Math.max(0, Math.min(PLANNING_LIMITS.maxInstancesPerClass, populationLimit - others));
}
export function canResizeClass(blueprint, classId, count) {
  const agent = blueprint.agent_classes.find((item) => item.id === classId);
  if (!agent || !Number.isInteger(count) || count < 0) return false;
  const target = Math.min(classInstanceLimit(blueprint, classId), count);
  const currentIds = identities(blueprint).filter((item) => item.classId === classId).map((item) => item.id);
  if (target === 0 || (agent.members?.length && target < agent.instances)) {
    const removed = new Set(currentIds.slice(target));
    if (removed.has(blueprint.topology.entrypoint)
        || blueprint.flows.some((flow) => flow.steps.some((step) => removed.has(step.agent)))) return false;
    if (target === 0 && !blueprint.agent_classes.some((item) => item.id !== classId && item.instances > 0)) return false;
  }
  return true;
}
export function resizeClass(blueprint, classId, count) {
  const next = clone(blueprint);
  const agent = next.agent_classes.find((c) => c.id === classId);
  if (!agent || !canResizeClass(next, classId, count) || agent.instances === count) return next;
  agent.instances = Math.min(classInstanceLimit(next, classId), count);
  if (agent.instances === 0) {
    const removed = new Set(identities(blueprint).filter((item) => item.classId === classId).map((item) => item.id));
    agent.members = [];
    next.topology.edges = next.topology.edges.filter((edge) => !removed.has(edge.a) && !removed.has(edge.b));
    return next;
  }
  if (agent.members?.length) {
    const removed = agent.members.slice(agent.instances).map((member) => `${classId}:${member.key}`);
    agent.members = agent.members.slice(0, agent.instances);
    while (agent.members.length < agent.instances) {
      const key = uniqueId("member", agent.members.map((member) => member.key));
      agent.members.push({ key, name: `${agent.name} ${agent.members.length + 1}`,
        instructions: "The owner must assign a distinct responsibility to this member before using the expanded team." });
    }
    const replacement = `${classId}:${agent.members[0].key}`;
    if (removed.includes(next.topology.entrypoint)) next.topology.entrypoint = replacement;
    next.topology.edges = next.topology.edges.filter((edge) => !removed.includes(edge.a) && !removed.includes(edge.b));
    for (const flow of next.flows) for (const step of flow.steps)
      if (removed.includes(step.agent)) step.agent = replacement;
    return next;
  }
  const rewrite = (identity) => {
    if (!identity.startsWith(`${classId}:`)) return identity;
    const suffix = identity.slice(classId.length + 1);
    const index = suffix === "main" ? 1 : Number(suffix);
    return `${classId}:${agent.instances === 1 ? "main" : Math.min(index || 1, agent.instances)}`;
  };
  next.topology.entrypoint = rewrite(next.topology.entrypoint);
  next.topology.edges = dedupeEdges(
    next.topology.edges.map((e) => ({ a: rewrite(e.a), b: rewrite(e.b) })),
  );
  for (const flow of next.flows)
    for (const step of flow.steps) step.agent = rewrite(step.agent);
  return next;
}
export function dedupeEdges(edges) {
  const seen = new Set();
  return edges.filter((e) => {
    const key = [e.a, e.b].sort().join("\0");
    if (e.a === e.b || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
export function removeClass(blueprint, classId) {
  if (blueprint.agent_classes.length === 1
      || !blueprint.agent_classes.some((item) => item.id !== classId && item.instances > 0)) return clone(blueprint);
  const next = clone(blueprint);
  next.agent_classes = next.agent_classes.filter((c) => c.id !== classId);
  const allIds = new Set(identities(next).map((a) => a.id));
  if (!allIds.has(next.topology.entrypoint))
    next.topology.entrypoint = [...allIds][0];
  next.topology.edges = next.topology.edges.filter(
    (e) => allIds.has(e.a) && allIds.has(e.b),
  );
  for (const flow of next.flows) {
    flow.steps = flow.steps.filter((s) => allIds.has(s.agent));
    const steps = new Set(flow.steps.map((s) => s.id));
    for (const step of flow.steps)
      step.depends_on = step.depends_on.filter((id) => steps.has(id));
  }
  next.flows = next.flows.filter((f) => f.steps.length);
  return next;
}
export function removeConnection(blueprint, connectionId) {
  const next = clone(blueprint);
  next.connections = next.connections.filter((c) => c.id !== connectionId);
  for (const agent of next.agent_classes)
    agent.tools.mcp = agent.tools.mcp.filter(
      (g) => g.connection_id !== connectionId,
    );
  return next;
}
export function removeStep(flow, stepId) {
  return {
    ...flow,
    steps: flow.steps
      .filter((s) => s.id !== stepId)
      .map((s) => ({
        ...s,
        depends_on: s.depends_on.filter((id) => id !== stepId),
      })),
  };
}
export function hasCycle(steps) {
  const visiting = new Set(),
    visited = new Set(),
    byId = new Map(steps.map((s) => [s.id, s]));
  function visit(id) {
    if (visiting.has(id)) return true;
    if (visited.has(id)) return false;
    visiting.add(id);
    for (const parent of byId.get(id)?.depends_on || [])
      if (visit(parent)) return true;
    visiting.delete(id);
    visited.add(id);
    return false;
  }
  return steps.some((s) => visit(s.id));
}
export function blueprintDiff(before, after) {
  const rows = [];
  const compare = (label, a, b) => {
    if (canonicalJson(a) !== canonicalJson(b)) rows.push(label);
  };
  compare("Project and purpose", before.project, after.project);
  const original = new Map(before.agent_classes.map((c) => [c.id, c]));
  if (
    before.agent_classes.length === after.agent_classes.length &&
    after.agent_classes.every((c) => original.has(c.id))
  )
    compare(
      "Agent order",
      before.agent_classes.map((c) => c.id),
      after.agent_classes.map((c) => c.id),
    );
  for (const c of after.agent_classes) {
    const prior = original.get(c.id);
    if (!prior)
      rows.push(
        `Add ${c.name} (${c.instances} ${c.instances === 1 ? "agent" : "agents"})`,
      );
    else compare(`Update ${c.name}`, prior, c);
    original.delete(c.id);
  }
  for (const c of original.values()) rows.push(`Remove ${c.name}`);
  compare("Tool connections", before.connections, after.connections);
  compare("Communication topology", before.topology, after.topology);
  compare("Workflows", before.flows, after.flows);
  compare("Knowledge base", before.knowledge, after.knowledge);
  compare(
    "Deployment settings",
    [before.target, before.gcp, before.advanced],
    [after.target, after.gcp, after.advanced],
  );
  return rows;
}
export function buildTopologyNodes(blueprint) {
  const edges = resolvedEdges(blueprint);
  return identities(blueprint).map((a) => ({
    identity: a.id,
    label: a.name,
    role: a.classId,
    group: "Your deployment",
    wired_to: edges.flatMap((e) =>
      e.a === a.id ? [e.b] : e.b === a.id ? [e.a] : [],
    ),
    state: "draft",
    addressable: false,
    presentation: { caption: a.role, section: a.classId },
  }));
}
export function normalizeError(
  data,
  fallback = "The request could not be completed.",
) {
  const detail = data?.detail || data?.error || data?.message;
  if (typeof detail === "string") return detail;
  if (Array.isArray(detail))
    return detail
      .map(
        (d) =>
          `${(d.loc || []).filter((x) => x !== "body").join(".")}: ${d.msg || d.message || "Invalid value"}`,
      )
      .join("\n");
  return fallback;
}
export function request(url, options = {}) {
  const method = options.method === undefined ? "GET" : String(options.method).toUpperCase();
  const readOptions = method === "GET" && !("timeoutMs" in options)
    ? { ...options, timeoutMs: 30000 } : options;
  return requestBody(url, readOptions, response => response.json());
}
export function requestBlob(url, options = {}) {
  return requestBody(url, options, response => response.blob());
}
async function requestBody(url, options, decode) {
  // Reads may opt into a deadline. Mutating requests retain their normal lifecycle.
  const { timeoutMs, stillCurrent, ...fetchOptions } = options;
  const timed = Number.isFinite(timeoutMs) && timeoutMs > 0;
  const controller = timed ? new AbortController() : null;
  const signal = fetchOptions.signal;
  const forwardAbort = () => controller.abort(signal.reason);
  if (controller && signal) {
    if (signal.aborted) forwardAbort();
    else signal.addEventListener("abort", forwardAbort, { once: true });
  }
  let timer;
  const deadline = timed ? new Promise((_, reject) => {
    timer = setTimeout(() => {
      const error = new Error("The server is taking too long to respond. Try again.");
      error.code = "read_timeout";
      reject(error);
      controller.abort(error);
    }, timeoutMs);
  }) : null;
  const read = async () => {
    const result = await fetch(
      url,
      fetchOptions.body
        ? {
            ...fetchOptions,
            ...(controller ? { signal: controller.signal } : {}),
            headers: { "Content-Type": "application/json", ...fetchOptions.headers },
            body: JSON.stringify(fetchOptions.body),
          }
        : controller ? { ...fetchOptions, signal: controller.signal } : fetchOptions,
    );
    if (stillCurrent) {
      if (controller?.signal.aborted || signal?.aborted) throw controller?.signal.reason ?? signal.reason;
      const admitted = await stillCurrent();
      if (controller?.signal.aborted || signal?.aborted) throw controller?.signal.reason ?? signal.reason;
      if (!admitted) return;
    }
    if (!result.ok) {
      let body = null;
      try {
        body = await result.json();
      } catch {}
      const error = new Error(
        normalizeError(body, `Request failed (${result.status}).`),
      );
      error.status = result.status;
      error.code = typeof body?.code === "string" ? body.code : null;
      if (error.code === "remote_product_action_not_sent") {
        error.action = typeof body?.action === "string" ? body.action : null;
        error.action_request_id = typeof body?.action_request_id === "string" ? body.action_request_id : null;
      }
      throw error;
    }
    return decode(result);
  };
  try {
    return await (deadline ? Promise.race([read(), deadline]) : read());
  } finally {
    clearTimeout(timer);
    if (controller && signal) signal.removeEventListener("abort", forwardAbort);
  }
}
export function flowLayout(steps) {
  const byId = new Map(steps.map((s) => [s.id, s])),
    depth = new Map();
  function level(id, visiting = new Set()) {
    if (depth.has(id)) return depth.get(id);
    if (visiting.has(id)) return 0;
    const next = new Set(visiting).add(id),
      dependencies = byId.get(id)?.depends_on || [];
    const value = dependencies.length
      ? Math.max(...dependencies.map((dep) => level(dep, next))) + 1
      : 0;
    depth.set(id, value);
    return value;
  }
  const levels = [];
  for (const step of steps) {
    const d = level(step.id);
    (levels[d] ||= []).push(step);
  }
  const width = Math.max(1, ...levels.map((row) => row.length)) * 230;
  const nodes = levels.flatMap((row, r) =>
    row.map((step, i) => ({
      step,
      x: (width - row.length * 230) / 2 + i * 230 + 10,
      y: r * 110 + 30,
    })),
  );
  return { nodes, width, height: levels.length * 110 + 10 };
}
export function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(",")}}`;
}
export async function fingerprint(value) {
  const ascii = canonicalJson(value).replace(
    /[\u007f-\uffff]/g,
    (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );
  const bytes = new TextEncoder().encode(ascii);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)]
    .map((n) => n.toString(16).padStart(2, "0"))
    .join("");
}
