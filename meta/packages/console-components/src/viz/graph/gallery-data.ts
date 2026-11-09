// Demo data for the graph gallery (dev only; nothing in the product imports
// it): a team in lanes, a plan in goal frames, an ego graph, and large
// populations for the clusters layout and the render-time measurements.
// Deterministic: the same picture on every load.

import type { VizEdge, VizFrame, VizLane, VizNode } from "./types";

/** A small seeded generator (mulberry32). */
export function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface DemoGraph {
  nodes: VizNode[];
  edges: VizEdge[];
  lanes?: VizLane[];
  frames?: VizFrame[];
}

function edge(from: string, to: string, relation: VizEdge["relation"], extra: Partial<VizEdge> = {}): VizEdge {
  return { id: `${from}>${to}:${relation}`, from, to, relation, ...extra };
}

/** The operations team of the Topology reference: lanes, every relation, live state and badges. */
export function demoTeam(): DemoGraph {
  const lanes: VizLane[] = [
    { id: "channels", label: "Channels" },
    { id: "personal", label: "Personal" },
    { id: "coordinators", label: "Coordinators" },
    { id: "domains", label: "Domains" },
    { id: "internal", label: "Internal" },
    { id: "shared", label: "Shared" },
    { id: "other-teams", label: "Other teams" },
  ];
  const nodes: VizNode[] = [
    { id: "billing-events", kind: "source", label: "billing events", lane: "channels", order: 0, sublabel: "Webhook" },
    { id: "support-inbox", kind: "source", label: "support inbox", lane: "channels", order: 1, sublabel: "Email", live: true },
    { id: "nightly-digest", kind: "source", label: "nightly digest", lane: "channels", order: 2, sublabel: "Schedule" },
    { id: "operator", kind: "person", label: "Operator", lane: "personal", order: 0, sublabel: "Console user" },
    { id: "desk", kind: "agent", label: "Desk assistant", lane: "personal", order: 1, sublabel: "Personal", status: "idle", memory: "idle" },
    { id: "router", kind: "coordinator", label: "Router", lane: "coordinators", order: 0, sublabel: "Coordinator", status: "idle", memory: "idle", hub: true },
    { id: "triage", kind: "coordinator", label: "Triage", lane: "coordinators", order: 1, sublabel: "Coordinator", status: "working", statusLabel: "Working", live: true, memory: "idle", hub: true },
    { id: "billing", kind: "agent", label: "Billing", lane: "domains", order: 0, sublabel: "Lead · Domain", status: "idle", memory: "idle" },
    { id: "delivery", kind: "agent", label: "Delivery", lane: "domains", order: 1, sublabel: "Domain", status: "degraded", tone: "warn" },
    { id: "support", kind: "agent", label: "Support", lane: "domains", order: 2, sublabel: "Domain", status: "idle", memory: "idle" },
    { id: "returns", kind: "agent", label: "Returns", lane: "domains", order: 3, sublabel: "Domain", status: "idle" },
    { id: "shipping", kind: "agent", label: "Shipping", lane: "domains", order: 4, sublabel: "Domain", status: "idle" },
    { id: "ticket-4411", kind: "helper", label: "ticket 4411", lane: "domains", anchor: "support", sublabel: "Helper of Support", status: "working", live: true },
    { id: "refund-check", kind: "helper", label: "refund check", lane: "domains", anchor: "billing", sublabel: "Side chat of Billing", status: "idle" },
    { id: "gate", kind: "agent", label: "Gate", lane: "internal", order: 0, sublabel: "Internal", status: "idle", memory: "idle" },
    { id: "health", kind: "agent", label: "Health Monitor", lane: "internal", order: 1, sublabel: "Internal", status: "idle" },
    { id: "auditor", kind: "agent", label: "Auditor", lane: "internal", order: 2, sublabel: "Internal", status: "wedged", tone: "crit" },
    { id: "team-memory", kind: "store", label: "Team memory", lane: "shared", sublabel: "Memory store" },
    { id: "finance", kind: "external", label: "Finance team", lane: "other-teams", sublabel: "Other team" },
    { id: "partner", kind: "unknown", label: "Partner runtime", lane: "other-teams", sublabel: "Not observed" },
  ];
  const edges: VizEdge[] = [
    edge("router", "triage", "wired"),
    edge("router", "billing", "wired"),
    edge("router", "delivery", "wired"),
    edge("router", "support", "wired"),
    edge("triage", "support", "wired"),
    edge("triage", "returns", "wired"),
    edge("gate", "billing", "wired"),
    edge("gate", "delivery", "wired"),
    edge("shipping", "delivery", "wired"),
    edge("health", "gate", "wired"),
    edge("auditor", "gate", "wired"),
    edge("billing", "shipping", "wired", { title: "Billing and Shipping · wired" }),
    edge("returns", "shipping", "wired", { state: "declared", title: "Returns and Shipping · declared, not connected" }),
    edge("desk", "router", "wired", { state: "pending", title: "Desk assistant and Router · Connecting" }),
    edge("triage", "support", "traffic", { weight: 12, directed: true, label: "12/h", title: "Triage to Support · 12 messages in the last hour · last 2m ago" }),
    edge("router", "billing", "traffic", { weight: 3, directed: true, label: "3/h", title: "Router to Billing · 3 messages in the last hour" }),
    edge("operator", "desk", "traffic", { weight: 2, directed: true, title: "Operator to Desk assistant · 2 messages" }),
    edge("billing-events", "billing", "route", { title: "billing events to Billing · route" }),
    edge("support-inbox", "triage", "route", { title: "support inbox to Triage · route" }),
    edge("nightly-digest", "health", "route", { state: "failed", title: "nightly digest to Health Monitor · 3 deliveries failed" }),
    edge("support", "ticket-4411", "lineage", { title: "Support started ticket 4411" }),
    edge("billing", "refund-check", "lineage", { title: "Side chat of Billing" }),
    edge("billing", "finance", "cross", { title: "Billing and Finance team · other team" }),
  ];
  return { nodes, edges, lanes };
}

/** The plan of the WorkGraph reference: goals as frames, a critical path, a collapsed done goal. */
export function demoPlan(): DemoGraph & { criticalNodes: string[]; criticalEdges: string[] } {
  const frames: VizFrame[] = [
    { id: "q3", label: "Q3 incident follow-ups", meta: "4 of 4 done · collapsed", collapsed: true, order: 2 },
    { id: "other", label: "Other work", order: 9 },
  ];
  const nodes: VizNode[] = [
    {
      id: "ship", kind: "goal", label: "Ship the 2.4 release", status: "in_progress", order: 0, sublabel: "1 of 5 done",
      progress: { done: 1, inProgress: 1, blocked: 1, total: 5 }, trailing: { text: "Coordinator · Leading the work", avatar: "C" },
    },
    { id: "freeze", kind: "item", label: "Freeze the release branch", frame: "ship", status: "completed", sublabel: "Release Manager · done 6h ago", order: 0 },
    { id: "notes", kind: "item", label: "Write the customer release notes", frame: "ship", status: "ready", sublabel: "Unassigned · Ready", order: 1 },
    { id: "run", kind: "item", label: "Run the full regression suite", frame: "ship", status: "in_progress", live: true, sublabel: "Research Analyst · 70%", avatar: "R" },
    { id: "publish", kind: "item", label: "Publish the release to all regions", frame: "ship", status: "blocked", sublabel: "High · Blocked by Run the full regression suite" },
    { id: "announce", kind: "item", label: "Announce the release to customers", frame: "ship", status: "open", sublabel: "Low · Unassigned" },
    { id: "docs", kind: "goal", label: "Refresh the docs", frame: "ship", status: "open", sublabel: "0 of 2 done", progress: { done: 0, total: 2 } },
    { id: "shots", kind: "item", label: "Retake the console screenshots", frame: "docs", status: "open", sublabel: "Unassigned · 1d" },
    { id: "proof", kind: "item", label: "Proofread the upgrade guide", frame: "docs", status: "ready", sublabel: "Release Manager · Ready" },
    {
      id: "vendor", kind: "goal", label: "Quarterly vendor review", status: "in_progress", order: 1, sublabel: "0 of 2 done",
      progress: { done: 0, inProgress: 1, total: 2 }, trailing: { text: "Research Analyst · Paused until Sun 18:51", avatar: "R" },
    },
    { id: "collect", kind: "item", label: "Collect vendor scorecards", frame: "vendor", status: "in_progress", sublabel: "Research Analyst · 2 of 5", avatar: "R" },
    { id: "draft", kind: "item", label: "Draft the renewal recommendation", frame: "vendor", status: "open", sublabel: "Unassigned" },
    { id: "q3", kind: "goal", label: "Q3 incident follow-ups", status: "completed", order: 2, sublabel: "4 of 4 done · collapsed", progress: { done: 4, total: 4 } },
    { id: "q3-a", kind: "item", label: "Add the paging runbook", frame: "q3", status: "completed" },
    { id: "q3-b", kind: "item", label: "Fix the retry storm", frame: "q3", status: "completed" },
    { id: "keys", kind: "item", label: "Rotate the API keys", frame: "other", status: "failed", sublabel: "Gate · failed 2h ago" },
    { id: "dash", kind: "item", label: "Archive the old dashboards", frame: "other", status: "cancelled", sublabel: "Cancelled" },
  ];
  const edges: VizEdge[] = [
    edge("freeze", "run", "blocks", { label: "blocks" }),
    edge("run", "publish", "blocks", { label: "blocks" }),
    edge("publish", "announce", "blocks", { label: "blocks" }),
    edge("notes", "announce", "blocks", { label: "blocks" }),
    edge("collect", "draft", "blocks", { label: "blocks" }),
    edge("shots", "proof", "blocks", { label: "blocks" }),
    edge("keys", "publish", "related", { label: "related" }),
    edge("q3-b", "run", "related", { label: "derived from" }),
  ];
  return {
    nodes,
    edges,
    frames,
    criticalNodes: ["run", "publish", "announce"],
    criticalEdges: ["run>publish:blocks", "publish>announce:blocks"],
  };
}

/** Billing's ego graph: peers around it by relation. */
export function demoEgo(): DemoGraph {
  const team = demoTeam();
  const keep = new Set(["billing"]);
  const edges = team.edges.filter((candidate) => {
    const touches = candidate.from === "billing" || candidate.to === "billing";
    if (touches) {
      keep.add(candidate.from);
      keep.add(candidate.to);
    }
    return touches;
  });
  const nodes = team.nodes.filter((node) => keep.has(node.id)).map((node) => ({ ...node, lane: undefined, anchor: undefined }));
  return { nodes, edges };
}

const GROUPS = ["Platform", "Payments", "Support", "Growth", "Data", "Infra", "Security", "Research"];

/**
 * A large team: `count` agents in `groups` groups, each with a lead, long-lived
 * members, helpers and a few live; wired to their lead, with cross-group links.
 */
export function demoPopulation(count: number, groups = 8, seed = 7): DemoGraph {
  const random = seeded(seed);
  const lanes: VizLane[] = Array.from({ length: groups }, (_, index) => ({ id: `g${index}`, label: GROUPS[index % GROUPS.length] + (index >= GROUPS.length ? ` ${Math.floor(index / GROUPS.length) + 1}` : "") }));
  const nodes: VizNode[] = [];
  const edges: VizEdge[] = [];
  const members: string[][] = lanes.map(() => []);
  for (let index = 0; index < count; index += 1) {
    const group = index % groups;
    const lane = lanes[group].id;
    const first = index < groups;
    const id = `a${index}`;
    const helper = !first && random() < 0.22;
    nodes.push({
      id,
      kind: first ? "coordinator" : helper ? "helper" : "agent",
      label: first ? `${lanes[group].label} lead` : helper ? `helper ${index}` : `${lanes[group].label.toLowerCase()} ${index}`,
      lane,
      live: random() < 0.06,
      status: "idle",
    });
    if (!first) {
      edges.push(edge(`a${group}`, id, helper ? "lineage" : "wired"));
      const peers = members[group];
      if (peers.length > 2 && random() < 0.35) edges.push(edge(id, peers[Math.floor(random() * peers.length)], "wired"));
    }
    members[group].push(id);
  }
  const crossCount = Math.round(count * 0.25);
  for (let index = 0; index < crossCount; index += 1) {
    const a = Math.floor(random() * groups);
    let b = Math.floor(random() * groups);
    if (b === a) b = (b + 1) % groups;
    const from = members[a][Math.floor(random() * members[a].length)];
    const to = members[b][Math.floor(random() * members[b].length)];
    if (from !== to) edges.push(edge(from, to, random() < 0.3 ? "traffic" : "wired", { weight: 1 + Math.floor(random() * 8) }));
  }
  const seen = new Set<string>();
  return { nodes, edges: edges.filter((candidate) => !seen.has(candidate.id) && seen.add(candidate.id)), lanes };
}

/** Pairs a message stream can travel: traffic first, then wiring. */
export function messagePairs(graph: DemoGraph): [string, string][] {
  const traffic = graph.edges.filter((candidate) => candidate.relation === "traffic").map((candidate) => [candidate.from, candidate.to] as [string, string]);
  const wired = graph.edges.filter((candidate) => candidate.relation === "wired" && !candidate.state).map((candidate) => [candidate.from, candidate.to] as [string, string]);
  return [...traffic, ...traffic, ...wired];
}
