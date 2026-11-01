// The topology panel's view model: the graph (data.ts) turned into what the
// graph kit draws (nodes, edges and lanes in the A3.9 grammar), the figures
// of the glance strip, the problems and the header's summary. Pure, so the
// panel's words and counts are tested without a renderer.

import type { Tone } from "../panel-kit/tone";
import { formatRelativeTime, humanizeEnum, plural } from "../panel-kit/format";
import type { VizEdge, VizLane, VizNode, VizNodeKind, VizTone } from "../viz/graph/types";
import type { TopoAgent, TopoEdge, TopoGraph, TopoSource } from "./data";
import type {
  ConsoleTopologyAgentStatus,
  ConsoleTopologyLineageKind,
  ConsoleTopologyMemoryScope,
  ConsoleTopologyMessage,
  ConsoleTopologyTraffic,
} from "./types";

const MINUTE = 60_000;

/** The traffic windows the overflow menu offers. */
export type TopologyWindowId = "15m" | "1h" | "24h";

export interface TopologyWindow {
  id: TopologyWindowId;
  ms: number;
  /** The menu's word. */
  label: string;
  /** The figure tile's label. */
  figure: string;
  /** "in the last hour". */
  phrase: string;
  /** "the hour before" (the delta's period). */
  previous: string;
  /** The edge label's unit: "12/h". */
  unit: string;
}

export const TOPOLOGY_WINDOWS: Record<TopologyWindowId, TopologyWindow> = {
  "15m": {
    id: "15m",
    ms: 15 * MINUTE,
    label: "15 minutes",
    figure: "Messages · 15 min",
    phrase: "in the last 15 minutes",
    previous: "the 15 minutes before",
    unit: "15m",
  },
  "1h": {
    id: "1h",
    ms: 60 * MINUTE,
    label: "1 hour",
    figure: "Messages · last hour",
    phrase: "in the last hour",
    previous: "the hour before",
    unit: "h",
  },
  "24h": {
    id: "24h",
    ms: 24 * 60 * MINUTE,
    label: "24 hours",
    figure: "Messages · 24 h",
    phrase: "in the last 24 hours",
    previous: "the day before",
    unit: "day",
  },
};

export const DEFAULT_TOPOLOGY_WINDOW: TopologyWindowId = "1h";

export function isTopologyWindowId(value: unknown): value is TopologyWindowId {
  return value === "15m" || value === "1h" || value === "24h";
}

export interface TopologyTrafficWindow {
  /** Messages per ordered pair inside the window. */
  traffic: ConsoleTopologyTraffic[];
  total: number;
  /** Messages in the window before it (the delta). */
  previousTotal: number;
  /** Messages per bin across the window, oldest first. */
  bins: number[];
}

/** Sum messages for a window ending now, with the window before it and a trend. */
export function topologyTrafficWindow(
  messages: readonly ConsoleTopologyMessage[],
  now: number,
  windowMs: number,
  binCount = 12,
): TopologyTrafficWindow {
  const start = now - windowMs;
  const previousStart = start - windowMs;
  const bins = new Array<number>(binCount).fill(0);
  const binMs = windowMs / binCount;
  const byPair = new Map<string, ConsoleTopologyTraffic>();
  let total = 0;
  let previousTotal = 0;
  for (const message of messages) {
    const count = message.count ?? 1;
    if (count <= 0 || message.from === message.to) continue;
    if (message.atMs >= previousStart && message.atMs < start) {
      previousTotal += count;
      continue;
    }
    if (message.atMs < start || message.atMs > now + MINUTE) continue;
    total += count;
    const bin = Math.min(binCount - 1, Math.max(0, Math.floor((message.atMs - start) / binMs)));
    bins[bin] += count;
    const key = `${message.from}>${message.to}`;
    const entry = byPair.get(key) || { from: message.from, to: message.to, count: 0, lastAtMs: 0 };
    entry.count += count;
    entry.lastAtMs = Math.max(entry.lastAtMs || 0, message.atMs);
    byPair.set(key, entry);
  }
  return { traffic: Array.from(byPair.values()), total, previousTotal, bins };
}

/** The node's state when the host supplies no status rule (the owner's typed facts first). */
export function fallbackTopologyStatus(agent: TopoAgent, runOpen: boolean): ConsoleTopologyAgentStatus {
  const state = agent.state;
  const retired = state === "retired" || state === "retiring" || state === "stopped" || state === "completed";
  const health = agent.agent?.progress?.health;
  const stuck = state === "broken" || health === "wedged";
  const phase = agent.responsePhase;
  const working = !retired && !stuck && (
    phase === "waiting" || phase === "tool-executing" || phase === "generating"
    || agent.agent?.progress?.run_state === "run_open"
    || (runOpen && phase === undefined && agent.agent?.progress?.run_state !== "idle")
  );
  if (state === "broken") return { working, health: "crit", healthWord: "Broken", retired };
  if (health === "wedged") return { working, health: "crit", healthWord: "Stalled", retired };
  if (health === "degraded" || state.includes("degrade") || agent.labels.console_degraded === "true") {
    return { working, health: "warn", healthWord: "Degraded", retired };
  }
  return { working, health: null, healthWord: null, retired };
}

export interface TopologyProblem {
  id: string;
  tone: "warn" | "crit";
  /** "Delivery is degraded". */
  text: string;
  /** The summary's grouping word: "degraded", "stalled", "not wired", "route failing". */
  word: string;
  node?: string;
  edge?: string;
}

export interface TopologyCounts {
  /** Members and helpers in the team (not other teams' agents, not retired ones). */
  agents: number;
  longLived: number;
  helpers: number;
  working: number;
  otherTeams: number;
  /** Wired pairs (inside the team), connected or connecting. */
  wired: number;
  /** Pairs to other teams. */
  cross: number;
  /** Declared pairs that are not wired. */
  declared: number;
  /** Routes an operator configured into an agent. */
  routes: number;
  /** Routes the runtime keeps for every member on its own (not counted as connections). */
  rosterRoutes: number;
  /** Helpers drawn with their parent. */
  lineage: number;
  /** Ordered pairs that exchanged messages in the window. */
  trafficPairs: number;
}

export interface TopologyViewOptions {
  /** The node's state (the host's rule, or `fallbackTopologyStatus`). */
  statusOf: (agent: TopoAgent) => ConsoleTopologyAgentStatus;
  /** The section (lane) an agent belongs to. */
  sectionOf: (agent: TopoAgent) => string;
  /** Section order, top to bottom; sections not listed follow by name. */
  sectionOrder?: readonly string[];
  showHelpers?: boolean;
  showRetired?: boolean;
  showChannels?: boolean;
  window?: TopologyWindowId;
  memoryScopes?: ReadonlyMap<string, ConsoleTopologyMemoryScope>;
  /**
   * The roles whose profile wires its members (`profile_capabilities.<role>.has_wiring`).
   * A lead or coordinator role draws as a coordinator only when its profile wires;
   * unknown (null) keeps the role words alone.
   */
  wiringRoles?: ReadonlySet<string> | null;
  /** Draw only the edges that touch these nodes (the Focus edge mode). */
  focus?: ReadonlySet<string> | null;
  /**
   * Nodes whose roster routes are drawn (the hovered or selected channel or
   * agent). A channel the runtime keeps for every member draws its routes
   * only on demand: a fan to every agent says nothing a sublabel cannot.
   */
  reveal?: ReadonlySet<string> | null;
  now?: number;
}

export interface TopologyViewModel {
  nodes: VizNode[];
  edges: VizEdge[];
  lanes: VizLane[];
  counts: TopologyCounts;
  problems: TopologyProblem[];
  /** Any relation at all (wired, declared, cross, routes, helpers or messages); roster routes do not count. */
  hasRelations: boolean;
  statusById: Map<string, ConsoleTopologyAgentStatus>;
}

export const TOPOLOGY_LANE_CHANNELS = "channels";
export const TOPOLOGY_LANE_SHARED = "shared";
export const TOPOLOGY_LANE_OTHER_TEAMS = "other-teams";

const LINEAGE_WORDS: Record<ConsoleTopologyLineageKind, string> = {
  side_chat: "side chat",
  fork: "fork",
  spawn: "helper",
  delegate: "delegate",
};

export function topologyLineageWord(kind: ConsoleTopologyLineageKind | undefined): string {
  return kind ? LINEAGE_WORDS[kind] : "helper";
}

/**
 * A node's mark: coordinators by section, or by a lead or coordinator role
 * whose profile wires its members; helpers, other teams, unobserved.
 */
function vizKind(agent: TopoAgent, section: string, wiringRoles?: ReadonlySet<string> | null): VizNodeKind {
  if (agent.kind === "external") return "external";
  if (agent.kind === "helper") return "helper";
  if (agent.kind === "unknown") return "unknown";
  if (/coordinat/i.test(section)) return "coordinator";
  const role = agent.role.toLowerCase();
  const leads = role.includes("coordinator") || role === "lead" || role.includes("orchestrator");
  if (leads && (!wiringRoles || wiringRoles.has(agent.role))) return "coordinator";
  return "agent";
}

function nodeStatusWord(status: ConsoleTopologyAgentStatus): { tone: VizTone; status: string; word: string } {
  if (status.retired) return { tone: "neutral", status: "retired", word: "Retired" };
  if (status.health === "crit") return { tone: "crit", status: (status.healthWord || "Stalled").toLowerCase(), word: status.healthWord || "Stalled" };
  if (status.working) return { tone: "live", status: "working", word: "Working" };
  if (status.health === "warn") return { tone: "warn", status: (status.healthWord || "Degraded").toLowerCase(), word: status.healthWord || "Degraded" };
  return { tone: "neutral", status: "idle", word: "Idle" };
}

/** The node's state in one word and tone, as its pill and tooltip read it. */
export function topologyStatusPresentation(status: ConsoleTopologyAgentStatus): { tone: Tone | "live"; word: string } {
  const presentation = nodeStatusWord(status);
  return { tone: presentation.tone, word: presentation.word };
}

function labelOf(graph: TopoGraph, id: string): string {
  return graph.byId.get(id)?.label || graph.sources.find((source) => source.id === id)?.label || id;
}

/** "Triage to Support · 12 messages in the last hour". */
export function topologyEdgeTitle(graph: TopoGraph, edge: TopoEdge, window: TopologyWindow, now?: number): string {
  const from = labelOf(graph, edge.from);
  const to = labelOf(graph, edge.to);
  switch (edge.relation) {
    case "traffic": {
      const forward = edge.counts?.forward ?? edge.weight ?? 0;
      const backward = edge.counts?.backward ?? 0;
      const parts = [backward > 0
        ? `${from} and ${to} · ${plural(forward + backward, "message")} ${window.phrase} (${forward} and ${backward})`
        : `${from} to ${to} · ${plural(forward, "message")} ${window.phrase}`];
      // "last 2m ago": when the newest of them was sent.
      const last = edge.lastAtMs && now ? formatRelativeTime(edge.lastAtMs, now) : "";
      if (last) parts.push(last === "just now" ? "the last one just now" : `last ${last}`);
      if (edge.wired) parts.push("wired");
      return parts.join(" · ");
    }
    case "route": {
      const routes = edge.routes || [];
      const parts = [`${edge.channel || from} routes to ${to}`];
      if (edge.managed) parts.push("every member's own route");
      else if (routes.length > 1) parts.push(plural(routes.length, "route"));
      if (edge.sentDeliveries || edge.failedDeliveries) {
        parts.push([edge.sentDeliveries ? `${edge.sentDeliveries} sent` : "", edge.failedDeliveries ? `${edge.failedDeliveries} failed` : ""].filter(Boolean).join(", "));
      }
      return parts.join(" · ");
    }
    case "lineage":
      return `${from} started ${to} · ${topologyLineageWord(edge.lineage)}`;
    case "cross":
    case "wired": {
      const parts = [`${from} and ${to}`];
      if (edge.relation === "cross") parts.push("another team");
      if (edge.state === "pending") parts.push(edge.pendingAction === "disconnect" ? "disconnecting" : "connecting");
      else if (edge.degraded) parts.push("declared, not wired");
      else if (edge.suppressed) parts.push("declared, disconnected by an operator");
      else if (edge.conflict) parts.push("wired, though an operator asked to disconnect");
      else parts.push(edge.operatorAdded ? "wired by an operator" : "wired");
      return parts.join(" · ");
    }
  }
}

function sourceSublabel(source: TopoSource, members: number): string {
  if (source.managed && members > 0 && source.recipients.length >= members) return "every member";
  return plural(source.recipients.length, "agent");
}

/** A roster channel names whom it reaches on the map; its fan of routes draws on hover. */
function sourceLabel(source: TopoSource, members: number): string {
  return source.managed ? `${source.label} · ${sourceSublabel(source, members)}` : source.label;
}

/** Build the drawable model. */
export function buildTopologyView(graph: TopoGraph, options: TopologyViewOptions): TopologyViewModel {
  const window = TOPOLOGY_WINDOWS[options.window || DEFAULT_TOPOLOGY_WINDOW];
  const showHelpers = options.showHelpers !== false;
  const showChannels = options.showChannels !== false;
  const statusById = new Map<string, ConsoleTopologyAgentStatus>();
  const sectionById = new Map<string, string>();
  for (const agent of graph.agents) {
    statusById.set(agent.id, options.statusOf(agent));
    if (agent.kind !== "external") sectionById.set(agent.id, options.sectionOf(agent) || "Other");
  }
  // A helper sits in its host's lane.
  for (const agent of graph.agents) {
    if (agent.kind === "helper" && agent.parent && sectionById.has(agent.parent)) {
      sectionById.set(agent.id, sectionById.get(agent.parent)!);
    }
  }

  const visibleAgents = graph.agents.filter((agent) => showHelpers || agent.kind !== "helper");
  const visible = new Set(visibleAgents.map((agent) => agent.id));
  const members = graph.agents.filter((agent) => agent.kind !== "helper" && agent.kind !== "external" && !statusById.get(agent.id)?.retired).length;

  // Nodes.
  const nodes: VizNode[] = [];
  const sections = new Set<string>();
  let hasExternal = false;
  for (const agent of visibleAgents) {
    const status = statusById.get(agent.id)!;
    const section = sectionById.get(agent.id);
    const presentation = nodeStatusWord(status);
    const scope = options.memoryScopes?.get(agent.ref.identity || agent.id) || options.memoryScopes?.get(agent.id);
    const lane = agent.kind === "external" ? TOPOLOGY_LANE_OTHER_TEAMS : section || "Other";
    if (agent.kind === "external") hasExternal = true;
    else sections.add(lane);
    const kind = vizKind(agent, section || "", options.wiringRoles);
    const degree = graph.degree[agent.id] || 0;
    nodes.push({
      id: agent.id,
      kind,
      label: agent.label,
      sublabel: agent.kind === "external"
        ? `${humanizeEnum(agent.role) || "Agent"} · ${agent.authority || "another team"}`
        : agent.kind === "helper" && agent.parent
          ? `${humanizeEnum(topologyLineageWord(agent.lineage))} of ${labelOf(graph, agent.parent)}`
          : joinWords(humanizeEnum(agent.role), section),
      lane,
      anchor: agent.kind === "helper" ? agent.parent : undefined,
      tone: presentation.tone,
      status: presentation.status,
      statusLabel: presentation.word,
      live: status.working,
      retired: status.retired || undefined,
      badge: status.health && !status.retired ? status.health : null,
      memory: scope && scope.records > 0 ? "idle" : "none",
      title: [
        agent.label,
        agent.kind === "external" ? `${humanizeEnum(agent.role) || "Agent"} in ${agent.authority || "another team"}` : joinWords(humanizeEnum(agent.role), section),
        presentation.word,
        plural(degree, "connection"),
      ].filter(Boolean).join("\n"),
      attributes: { "data-topology-kind": agent.kind },
      data: agent,
    });
  }
  for (const source of showChannels ? graph.sources : []) {
    nodes.push({
      id: source.id,
      kind: "source",
      label: sourceLabel(source, members),
      sublabel: sourceSublabel(source, members),
      lane: TOPOLOGY_LANE_CHANNELS,
      tone: source.routes.some((route) => (route.deliveries?.failed || 0) > 0) ? "crit" : "neutral",
      title: [`${source.label} channel`, `Routes to ${sourceSublabel(source, members)}`].join("\n"),
      attributes: { "data-topology-kind": "source" },
      data: source,
    });
  }
  for (const store of graph.stores) {
    nodes.push({
      id: `store:${store.id}`,
      kind: "store",
      label: store.label,
      sublabel: typeof store.records === "number" ? plural(store.records, "record") : undefined,
      lane: TOPOLOGY_LANE_SHARED,
      attributes: { "data-topology-kind": "store" },
      data: store,
    });
  }

  // Lanes, in the section order.
  const order = options.sectionOrder || [];
  const sectionLanes = Array.from(sections).sort((left, right) => {
    const li = order.indexOf(left);
    const ri = order.indexOf(right);
    if (li !== ri) return (li === -1 ? Number.MAX_SAFE_INTEGER : li) - (ri === -1 ? Number.MAX_SAFE_INTEGER : ri);
    return left.localeCompare(right);
  });
  const lanes: VizLane[] = [];
  if (showChannels && graph.sources.length > 0) lanes.push({ id: TOPOLOGY_LANE_CHANNELS, label: "Channels" });
  for (const section of sectionLanes) lanes.push({ id: section, label: section });
  if (graph.stores.length > 0) lanes.push({ id: TOPOLOGY_LANE_SHARED, label: "Shared" });
  if (hasExternal) lanes.push({ id: TOPOLOGY_LANE_OTHER_TEAMS, label: "Other teams" });

  const edges = topologyVizEdges(graph, options);

  // Counts.
  const counts: TopologyCounts = {
    agents: 0,
    longLived: 0,
    helpers: 0,
    working: 0,
    otherTeams: 0,
    wired: 0,
    cross: 0,
    declared: 0,
    routes: 0,
    rosterRoutes: 0,
    lineage: 0,
    trafficPairs: 0,
  };
  for (const agent of graph.agents) {
    const status = statusById.get(agent.id)!;
    if (agent.kind === "external") {
      counts.otherTeams += 1;
      continue;
    }
    if (status.retired) continue;
    counts.agents += 1;
    if (agent.kind === "helper") counts.helpers += 1;
    else counts.longLived += 1;
    if (status.working) counts.working += 1;
  }
  for (const edge of graph.edges) {
    if (edge.relation === "wired" && edge.state !== "declared") counts.wired += 1;
    else if (edge.relation === "cross" && edge.state !== "declared") counts.cross += 1;
    else if ((edge.relation === "wired" || edge.relation === "cross") && edge.state === "declared") counts.declared += 1;
    else if (edge.relation === "route") {
      if (edge.managed) counts.rosterRoutes += 1;
      else counts.routes += 1;
    }
    else if (edge.relation === "lineage") counts.lineage += 1;
    else if (edge.relation === "traffic") counts.trafficPairs += 1;
  }

  // Problems.
  const problems: TopologyProblem[] = [];
  for (const agent of graph.agents) {
    const status = statusById.get(agent.id)!;
    if (!status.health || status.retired) continue;
    const word = (status.healthWord || (status.health === "crit" ? "Stalled" : "Degraded")).toLowerCase();
    problems.push({ id: `agent:${agent.id}`, tone: status.health, text: `${agent.label} is ${word}`, word, node: agent.id });
  }
  for (const edge of graph.edges) {
    if (edge.relation === "route" && edge.failedDeliveries) {
      problems.push({
        id: `route:${edge.id}`,
        tone: "crit",
        text: `${plural(edge.failedDeliveries, "delivery", "deliveries")} to ${labelOf(graph, edge.to)} failed`,
        word: edge.failedDeliveries === 1 ? "delivery failed" : "deliveries failed",
        edge: edge.id,
      });
    } else if (edge.degraded) {
      problems.push({
        id: `wiring:${edge.id}`,
        tone: "warn",
        text: `${labelOf(graph, edge.from)} and ${labelOf(graph, edge.to)} are not wired`,
        word: "not wired",
        edge: edge.id,
      });
    } else if (edge.conflict) {
      problems.push({
        id: `conflict:${edge.id}`,
        tone: "warn",
        text: `${labelOf(graph, edge.from)} and ${labelOf(graph, edge.to)} are still wired`,
        word: "still wired",
        edge: edge.id,
      });
    }
  }
  problems.sort((left, right) => (left.tone === right.tone ? 0 : left.tone === "crit" ? -1 : 1));

  // A roster channel's routes (the runtime's own route to every member)
  // connect no one: a team with only those has no connections yet.
  const hasRelations = graph.edges.some((edge) => !(edge.relation === "route" && edge.managed));
  return { nodes, edges, lanes, counts, problems, hasRelations, statusById };
}

/**
 * The edges to draw: every relation whose ends are shown, a roster
 * channel's routes only for the revealed nodes, and in the Focus edge mode
 * only the edges of the focused nodes. Separate from the nodes, so hovering
 * changes the edges and never the node list.
 */
export function topologyVizEdges(
  graph: TopoGraph,
  options: Pick<TopologyViewOptions, "showHelpers" | "showChannels" | "focus" | "reveal" | "window" | "now">,
): VizEdge[] {
  const window = TOPOLOGY_WINDOWS[options.window || DEFAULT_TOPOLOGY_WINDOW];
  const showHelpers = options.showHelpers !== false;
  const showChannels = options.showChannels !== false;
  const visible = new Set(graph.agents.filter((agent) => showHelpers || agent.kind !== "helper").map((agent) => agent.id));
  const shownSources = new Set(showChannels ? graph.sources.map((source) => source.id) : []);
  const managedSources = new Set(graph.sources.filter((source) => source.managed).map((source) => source.id));
  const edges: VizEdge[] = [];
  for (const edge of graph.edges) {
    const endsVisible = (visible.has(edge.from) || shownSources.has(edge.from)) && visible.has(edge.to);
    if (!endsVisible) continue;
    if (edge.relation === "lineage" && !showHelpers) continue;
    if (edge.relation === "route") {
      if (!shownSources.has(edge.from)) continue;
      if (managedSources.has(edge.from) && edge.managed) {
        const reveal = options.reveal;
        if (!reveal || !(reveal.has(edge.from) || reveal.has(edge.to))) continue;
      }
    }
    if (options.focus && !(options.focus.has(edge.from) || options.focus.has(edge.to))) continue;
    edges.push(toVizEdge(graph, edge, window, options.now));
  }
  return edges;
}

function joinWords(...parts: Array<string | null | undefined>): string {
  return parts.filter(Boolean).join(" · ");
}

function toVizEdge(graph: TopoGraph, edge: TopoEdge, window: TopologyWindow, now?: number): VizEdge {
  const base: VizEdge = {
    id: edge.id,
    from: edge.from,
    to: edge.to,
    relation: edge.relation,
    directed: edge.directed,
    state: edge.state,
    // A pair an operator disconnected on purpose is declared, not degraded:
    // no warn mark.
    ...(edge.suppressed ? { mark: null } : {}),
    title: topologyEdgeTitle(graph, edge, window, now),
    attributes: {
      "data-topology-relation": edge.relation,
      ...(edge.degraded ? { "data-degraded": "true" } : {}),
      ...(edge.suppressed ? { "data-suppressed": "true" } : {}),
      ...(edge.managed ? { "data-managed": "true" } : {}),
    },
    data: edge,
  };
  if (edge.relation === "traffic") {
    base.weight = edge.weight;
    base.label = `${edge.weight}/${window.unit}`;
  }
  if (edge.relation === "route" && edge.failedDeliveries) {
    base.label = `${edge.failedDeliveries} failed`;
  }
  return base;
}

export interface TopologySummaryFact {
  key: string;
  text: string;
  tone?: Tone | "live";
}

/** The header's live summary: "8 agents · 1 working now · 1 degraded". */
export function topologySummaryFacts(view: Pick<TopologyViewModel, "counts" | "problems">): TopologySummaryFact[] {
  const facts: TopologySummaryFact[] = [{ key: "agents", text: plural(view.counts.agents, "agent") }];
  if (view.counts.working > 0) facts.push({ key: "working", text: `${view.counts.working} working now`, tone: "live" });
  const byWord = new Map<string, { tone: "warn" | "crit"; count: number }>();
  for (const problem of view.problems) {
    const entry = byWord.get(problem.word) || { tone: problem.tone, count: 0 };
    entry.count += 1;
    if (problem.tone === "crit") entry.tone = "crit";
    byWord.set(problem.word, entry);
  }
  for (const [word, entry] of Array.from(byWord).slice(0, 2)) {
    facts.push({ key: `problem:${word}`, text: `${entry.count} ${word}`, tone: entry.tone });
  }
  return facts;
}

/** "7 wired · 3 routes · 1 helper", or "none". */
/**
 * The connections figure's words: what it counts, kind by kind ("7 wired ·
 * 3 routes · 1 helper"). A declared pair that is not wired is not a
 * connection; the problems figure names it.
 */
export function topologyConnectionWords(counts: TopologyCounts): string {
  const parts = [
    counts.wired ? `${counts.wired} wired` : "",
    counts.routes ? plural(counts.routes, "route") : "",
    counts.lineage ? plural(counts.lineage, "helper") : "",
    counts.cross ? plural(counts.cross, "other team", "other teams") : "",
  ].filter(Boolean);
  return parts.length ? parts.join(" · ") : "none";
}

/** The connections figure: wired pairs, pairs to other teams, routes and helpers. */
export function topologyConnectionCount(counts: TopologyCounts): number {
  return counts.wired + counts.cross + counts.routes + counts.lineage;
}
