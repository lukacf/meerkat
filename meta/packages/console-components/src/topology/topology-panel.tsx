import React from "react";

import {
  topologyEdgeKey,
  topologyMutationIntent,
  type TopologyEndpoint,
  type TopologyManagementState,
  type TopologyMutationIntent,
  type TopologyMutationKind,
  type TopologyOperationReceipt,
} from "@console-core";

import {
  Button,
  Chip,
  DrawerLayout,
  EmptyIllustration,
  EmptyState,
  IconButton,
  LoadingState,
  PanelBody,
  PanelHeader,
  PanelShell,
  StatusToast,
  standardPanelMenu,
  useNow,
  usePanelDensity,
  usePanelPreference,
  useShowTechnicalIds,
  type MenuEntry,
  type SegmentOption,
} from "../panel-kit";
import { KitGlyph } from "../panel-kit/glyphs";
import { GraphList, GraphView, graphSummary, type GraphViewHandle, type VizMemoryEvent, type VizMessage } from "../viz";
import type {
  ConsoleAgent,
  ConsoleFrame,
  ConsoleTopologyActivitySource,
  ConsoleTopologyAgentStatus,
  ConsoleTopologyGrouping,
  ConsoleTopologyLineage,
  ConsoleTopologyLink,
  ConsoleTopologyMemoryScope,
  ConsoleTopologyNode,
  ConsoleTopologyRoute,
  ConsoleTopologyRoutesAccess,
  ConsoleTopologyStore,
  TopologyEdgeRef,
  TopologyPanelView,
} from "./types";
import { RoleTree } from "./role-tree";
import {
  buildGraph,
  deriveTopologyActivity,
  deriveTopologyTraffic,
  withTopologyTraffic,
  type TopoAgent,
} from "./data";
import {
  ConnectionPicker,
  type TopologyBoundedAction,
} from "./connection-picker";
import { TopologyDrawer, topologyNowLine, type TopologyAgentActivity, type TopologyDrawerTarget } from "./topology-drawer";
import { TopologyGlance } from "./topology-glance";
import {
  DEFAULT_TOPOLOGY_WINDOW,
  TOPOLOGY_WINDOWS,
  buildTopologyView,
  fallbackTopologyStatus,
  isTopologyWindowId,
  topologySummaryFacts,
  topologyTrafficWindow,
  topologyVizEdges,
  type TopologyTrafficWindow,
  type TopologyWindowId,
} from "./view-model";

export interface TopologyPanelProps {
  nodes: ConsoleTopologyNode[];
  agents: ConsoleAgent[];
  /** Live frames: activity, open tools, memory uses, and (without an activity source) open runs and peer messages. */
  activity: ConsoleFrame[];
  title?: React.ReactNode;
  view?: TopologyPanelView;
  defaultView?: TopologyPanelView;
  onViewChange?: (view: TopologyPanelView) => void;
  /**
   * Trusted host/server state. Without it the topology remains passive even if
   * mutation callbacks are supplied.
   */
  management?: TopologyManagementState | null;
  connectionSourceId?: string | null;
  defaultConnectionSourceId?: string | null;
  onConnectionSourceChange?: (sourceId: string | null) => void;
  onRequestMutation?: (intent: TopologyMutationIntent) => void | Promise<void>;
  onRequestPairInspection?: (edge: TopologyEdgeRef) => void | Promise<void>;
  interactionMode?: "explicit" | "direct";
  resolvingPairKeys?: ReadonlySet<string>;
  onRetryOperation?: (receipt: TopologyOperationReceipt) => void | Promise<void>;
  bulkActions?: readonly TopologyBoundedAction[];
  onRequestBulkAction?: (action: TopologyBoundedAction) => void | Promise<void>;
  /** Open an agent's chat beside the map (double-click or Enter on a node, a role chip, Open chat). */
  onSelectAgent?: (agentId: string) => void;
  /** Open an agent's details (the drawer's Details). */
  onOpenAgentDetails?: (agentId: string) => void;
  /** Open Memory, at the agent's own memory when one is named. */
  onOpenMemory?: (agentId: string | null) => void;
  /** The team's display name for the role view's root ("Team" without one). */
  teamName?: string;
  /** The team's key (its id), the role view root's tooltip. */
  teamKey?: string;

  /** Every pair the topology query reports. When given, the query decides the wiring. */
  links?: readonly ConsoleTopologyLink[] | null;
  /** Routes from channels to agents. */
  routes?: readonly ConsoleTopologyRoute[] | null;
  /** Whether the routes read worked: `denied` hides CHANNELS and the legend says why. */
  routesAccess?: ConsoleTopologyRoutesAccess | null;
  /** Typed lineage (who started each helper, fork or side chat). */
  lineage?: readonly ConsoleTopologyLineage[] | null;
  /**
   * The console's activity history (its activity ledger): messages per pair
   * in the window, the trend, open runs and each agent's day. Without it the
   * panel reads what the live frames in `activity` show.
   */
  activitySource?: ConsoleTopologyActivitySource | null;
  /** The agents' own memory scopes (the memory arcs, the drawer's Memory). */
  memoryScopes?: readonly ConsoleTopologyMemoryScope[] | null;
  /** Memory stores for the SHARED lane (only when memory is available). */
  stores?: readonly ConsoleTopologyStore[] | null;
  /** The host's sections (the sidebar's), so lanes read the sidebar's words. */
  grouping?: ConsoleTopologyGrouping | null;
  /**
   * The host's one status rule for an agent; `openRun` says the console saw
   * a run of the agent start that has not ended.
   */
  statusOf?: ((agent: ConsoleAgent, openRun: boolean) => ConsoleTopologyAgentStatus) | null;

  /** When the data was read (the header's freshness). */
  updatedAtMs?: number | null;
  /** A refresh is running (the header hairline, "Updating"). */
  refreshing?: boolean;
  /** The first read has not answered yet: the graph skeleton. */
  loading?: boolean;
  /** The last refresh failed: a toast over the kept map. */
  error?: string | null;
  /** The live topology query is unavailable; the map shows the roster's last state. */
  liveUnavailable?: boolean;
  onRefresh?: () => void;
  /** The console can start members: the no-agents state offers it. */
  onCreateAgent?: (() => void) | null;
  /** The runtime can wire members: the no-connections state offers Connect agents. */
  canWireMembers?: boolean;
  /** Where "How wiring works" links. */
  wiringDocsHref?: string;
}

type EdgeMode = "all" | "focus";
type OnOff = "on" | "off";
const ON_OFF: readonly OnOff[] = ["on", "off"];
const WINDOW_IDS: readonly TopologyWindowId[] = ["15m", "1h", "24h"];

const VIEW_STORAGE = "meta-console-topology-view";
const EDGE_STORAGE = "meta-console-topology-edges";
const WINDOW_STORAGE = "meta-console-topology-window";
const SHOW_HELPERS_STORAGE = "meta-console-topology-show-helpers";
const SHOW_RETIRED_STORAGE = "meta-console-topology-show-retired";
const SHOW_CHANNELS_STORAGE = "meta-console-topology-show-channels";
const DAY_MS = 24 * 60 * 60_000;
/** Particles play the messages of the last few seconds only. */
const PARTICLE_WINDOW_MS = 12_000;
/** Above this many nodes the map draws groups as rings (the A3.9 level of detail). */
const CLUSTER_THRESHOLD = 150;
const WIRING_DOCS = "https://docs.rkat.ai/meta/topology";

const VIEW_OPTIONS: Array<{ id: TopologyPanelView; label: string; glyph: Parameters<typeof KitGlyph>[0]["name"]; help: string }> = [
  { id: "graph", label: "Lanes", glyph: "lanes", help: "The team in lanes, with every relation" },
  { id: "clusters", label: "Clusters", glyph: "clusters", help: "Agents grouped and pulled together by their relations" },
  { id: "roles", label: "Roles", glyph: "tree", help: "Agents by role" },
  { id: "list", label: "List", glyph: "list", help: "A table of agents and their connections" },
];

function readStoredView(hasConnectionView: boolean): TopologyPanelView | null {
  try {
    const stored = localStorage.getItem(VIEW_STORAGE);
    if (stored === "edit" || stored === "connections") return hasConnectionView ? "connections" : null;
    if (stored === "summary" || stored === "force" || stored === "bullseye") return "graph";
    if (stored === "graph" || stored === "roles" || stored === "clusters" || stored === "list") return stored;
  } catch { /* ignore */ }
  return null;
}

function writeStored(key: string, value: string): void {
  try { localStorage.setItem(key, value); } catch { /* ignore */ }
}

function readStoredEdgeMode(): EdgeMode {
  try {
    const stored = localStorage.getItem(EDGE_STORAGE);
    if (stored === "all" || stored === "focus") return stored;
  } catch { /* ignore */ }
  return "all";
}

/** A roster-like agent for a node the roster does not list (the section rule reads it). */
function agentLike(agent: TopoAgent): ConsoleAgent {
  return agent.agent || {
    identity: agent.ref.identity || agent.id,
    agent_id: agent.id,
    member_id: agent.id,
    label: agent.label,
    kind: "agent",
    role: agent.role,
    state: agent.state,
    group: agent.group,
    labels: agent.labels,
  };
}

/** The open tool per identity: the newest tool start with no completion after it. */
function openTools(frames: readonly ConsoleFrame[], resolve: (reference: string) => string | null): Map<string, { name: string; startedAtMs: number }> {
  const result = new Map<string, { name: string; startedAtMs: number }>();
  const closed = new Set<string>();
  for (const frame of frames) {
    const id = frame.identity ? resolve(frame.identity) : null;
    if (!id) continue;
    const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
    const callId = typeof data.tool_call_id === "string" ? data.tool_call_id : typeof data.id === "string" ? data.id : "";
    if (frame.event === "tool_execution_completed" || frame.event === "tool_result_received") {
      if (callId) closed.add(callId);
      continue;
    }
    if (frame.event !== "tool_execution_started" || result.has(id)) continue;
    if (callId && closed.has(callId)) continue;
    const name = typeof data.name === "string" ? data.name : typeof data.tool_name === "string" ? data.tool_name : "a tool";
    result.set(id, { name, startedAtMs: frame.timestampMs || 0 });
  }
  return result;
}

const subscribeNothing = () => () => {};
const versionZero = () => 0;

/** The window from the activity history: totals per pair, the window before, the trend. */
/** What a failed live read says: the map keeps the roster it had. */
const LIVE_UNAVAILABLE = "Live topology unavailable, showing the last roster";

function topologySourceWindow(source: ConsoleTopologyActivitySource, now: number, windowMs: number): TopologyTrafficWindow {
  const start = now - windowMs;
  const traffic = source.traffic({ sinceMs: start, untilMs: now }).map((entry) => ({
    from: entry.from,
    to: entry.to,
    count: entry.count,
    lastAtMs: entry.lastMs,
  }));
  const total = traffic.reduce((sum, entry) => sum + entry.count, 0);
  const previousTotal = source.traffic({ sinceMs: start - windowMs, untilMs: start })
    .reduce((sum, entry) => sum + entry.count, 0);
  const bins = source.series(null, "messagesSent", { bins: 12, binMs: windowMs / 12, endMs: now });
  return { traffic, total, previousTotal, bins };
}

/** An agent's turns over the last 24 hours, by hour (the drawer's activity). */
function topologyAgentDay(source: ConsoleTopologyActivitySource | null | undefined, identity: string, now: number): TopologyAgentActivity | null {
  if (!source) return null;
  const values = source.series(identity, "turnsStarted", { bins: 24, binMs: DAY_MS / 24, endMs: now });
  return { values, total: values.reduce((sum, value) => sum + value, 0), unit: "turns" };
}

function memoryOp(event: string): VizMemoryEvent["op"] | null {
  if (!event.startsWith("memory.")) return null;
  if (event.includes("quarantine")) return "quarantine";
  if (event.includes("write") || event.includes("record") || event.includes("promot")) return "write";
  if (event.includes("inject") || event.includes("read") || event.includes("recall") || event.includes("search")) return "read";
  return null;
}

export function TopologyPanel({
  nodes,
  agents,
  activity,
  title = "Topology",
  view: controlledView,
  defaultView,
  onViewChange,
  management,
  connectionSourceId,
  defaultConnectionSourceId,
  onConnectionSourceChange,
  onRequestMutation,
  onRequestPairInspection,
  interactionMode = "explicit",
  resolvingPairKeys,
  onRetryOperation,
  bulkActions,
  onRequestBulkAction,
  onSelectAgent,
  onOpenAgentDetails,
  onOpenMemory,
  teamName,
  teamKey,
  links,
  routes,
  routesAccess,
  lineage,
  activitySource,
  memoryScopes,
  stores,
  grouping,
  statusOf,
  updatedAtMs,
  refreshing = false,
  loading = false,
  error,
  liveUnavailable = false,
  onRefresh,
  onCreateAgent,
  canWireMembers = false,
  wiringDocsHref = WIRING_DOCS,
}: TopologyPanelProps): React.JSX.Element {
  const hasConnectionView = Boolean(management && management.policy.mode !== "disabled");

  // View, with the stored choice and the legacy names.
  const [uncontrolledView, setUncontrolledView] = React.useState<TopologyPanelView>(() => {
    if (defaultView && (defaultView !== "connections" || hasConnectionView)) return defaultView;
    return readStoredView(hasConnectionView) || "graph";
  });
  const requestedView = controlledView ?? uncontrolledView;
  const view: TopologyPanelView = requestedView === "connections" && !hasConnectionView ? "graph" : requestedView;
  const pickView = React.useCallback((next: TopologyPanelView) => {
    if (next === "connections" && !hasConnectionView) return;
    if (controlledView == null) {
      setUncontrolledView(next);
      writeStored(VIEW_STORAGE, next);
    }
    onViewChange?.(next);
  }, [controlledView, hasConnectionView, onViewChange]);

  const [edgeMode, setEdgeModeState] = React.useState<EdgeMode>(readStoredEdgeMode);
  const setEdgeMode = (next: EdgeMode) => {
    setEdgeModeState(next);
    writeStored(EDGE_STORAGE, next);
  };
  const [storedWindow, setStoredWindow] = usePanelPreference<TopologyWindowId>(WINDOW_STORAGE, DEFAULT_TOPOLOGY_WINDOW, WINDOW_IDS);
  const windowId: TopologyWindowId = isTopologyWindowId(storedWindow) ? storedWindow : DEFAULT_TOPOLOGY_WINDOW;
  const [showHelpers, setShowHelpers] = usePanelPreference<OnOff>(SHOW_HELPERS_STORAGE, "on", ON_OFF);
  const [showRetired, setShowRetired] = usePanelPreference<OnOff>(SHOW_RETIRED_STORAGE, "off", ON_OFF);
  const [showChannels, setShowChannels] = usePanelPreference<OnOff>(SHOW_CHANNELS_STORAGE, "on", ON_OFF);
  const show = { helpers: showHelpers === "on", retired: showRetired === "on", channels: showChannels === "on" };
  const [density, setDensity] = usePanelDensity("topology");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("topology");
  const now = useNow();

  // The graph: every node source and relation, keyed by identity.
  const baseGraph = React.useMemo(() => buildGraph(nodes, agents, {
    links: links ?? undefined,
    routes: routesAccess === "denied" ? [] : routes ?? undefined,
    lineage: lineage ?? undefined,
    operations: management?.operations,
    stores: stores ?? undefined,
  }), [agents, links, lineage, management?.operations, nodes, routes, routesAccess, stores]);

  // Messages: the activity history when the host has one, else what the
  // live frames show. Read again when the history changes, and on the
  // shared 15 s clock so the window slides.
  const sourceVersion = React.useSyncExternalStore(
    activitySource?.subscribe ?? subscribeNothing,
    activitySource ? () => activitySource.getVersion() : versionZero,
    activitySource ? () => activitySource.getVersion() : versionZero,
  );
  const frameMessages = React.useMemo(
    () => deriveTopologyTraffic(activity, baseGraph.resolve, 0).messages,
    [activity, baseGraph],
  );
  const windowMs = TOPOLOGY_WINDOWS[windowId].ms;
  const trafficWindow = React.useMemo(
    () => activitySource
      ? topologySourceWindow(activitySource, now, windowMs)
      : topologyTrafficWindow(frameMessages, now, windowMs),
    // `sourceVersion` re-reads the history when it counts something new.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [activitySource, frameMessages, now, sourceVersion, windowMs],
  );
  const graph = React.useMemo(
    () => withTopologyTraffic(baseGraph, trafficWindow.traffic),
    [baseGraph, trafficWindow.traffic],
  );

  // Live state: the host's status rule, else the owner's facts and open runs.
  const liveActivity = React.useMemo(() => deriveTopologyActivity(activity, baseGraph), [activity, baseGraph]);
  const statusFor = React.useCallback((agent: TopoAgent): ConsoleTopologyAgentStatus => {
    const identity = agent.ref.identity || agent.id;
    const openRun = activitySource ? activitySource.hasOpenRun(identity) : Boolean(liveActivity.busy[agent.id]);
    if (statusOf && agent.agent) return statusOf(agent.agent, openRun);
    return fallbackTopologyStatus(agent, openRun);
    // `sourceVersion` re-reads the open runs when the history changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activitySource, liveActivity.busy, sourceVersion, statusOf]);
  const sectionOf = React.useCallback((agent: TopoAgent): string => {
    if (grouping) return grouping.sectionOf(agentLike(agent)) || "Other";
    return agent.group || "Agents";
  }, [grouping]);
  const memoryById = React.useMemo(
    () => new Map((memoryScopes || []).map((scope) => [scope.identity, scope])),
    [memoryScopes],
  );

  // Selection, hover and the drawer.
  const [selected, setSelected] = React.useState<TopologyDrawerTarget | null>(null);
  const [hovered, setHovered] = React.useState<string | null>(null);
  const [connectFor, setConnectFor] = React.useState<string | null>(null);
  const graphRef = React.useRef<GraphViewHandle | null>(null);
  React.useEffect(() => {
    // A selection whose node or edge left the graph closes the drawer.
    if (!selected) return;
    const exists = selected.kind === "agent" ? graph.byId.has(selected.id)
      : selected.kind === "edge" ? graph.edges.some((edge) => edge.id === selected.id)
        : selected.kind === "source" ? graph.sources.some((source) => source.id === selected.id)
          : graph.stores.some((store) => `store:${store.id}` === selected.id);
    if (!exists) setSelected(null);
  }, [graph, selected]);
  const selectedNode = selected && selected.kind !== "edge" ? selected.id : null;
  const reveal = React.useMemo(() => new Set([hovered, selectedNode].filter((id): id is string => Boolean(id))), [hovered, selectedNode]);
  const focus = edgeMode === "focus" ? reveal : null;

  const showChannelsNow = show.channels && routesAccess !== "denied";
  // Nodes, lanes, counts and problems: independent of the hover.
  const model = React.useMemo(() => buildTopologyView(graph, {
    statusOf: statusFor,
    sectionOf,
    sectionOrder: grouping?.order,
    showHelpers: show.helpers,
    showRetired: show.retired,
    showChannels: showChannelsNow,
    window: windowId,
    memoryScopes: memoryById,
    now,
  }), [graph, grouping?.order, memoryById, now, sectionOf, showChannelsNow, show.helpers, show.retired, statusFor, windowId]);
  // Edges: a hover reveals a roster channel's routes and, in Focus mode,
  // picks the edges drawn.
  const edges = React.useMemo(() => topologyVizEdges(graph, {
    showHelpers: show.helpers,
    showChannels: showChannelsNow,
    window: windowId,
    focus,
    reveal,
    now,
  }), [focus, graph, now, reveal, showChannelsNow, show.helpers, windowId]);
  // The list names every relation of a row, a roster channel's routes too.
  const listEdges = React.useMemo(() => topologyVizEdges(graph, {
    showHelpers: show.helpers,
    showChannels: showChannelsNow,
    window: windowId,
    reveal: new Set(graph.sources.map((source) => source.id)),
  }), [graph, showChannelsNow, show.helpers, windowId]);

  // Particles and memory uses from the live frames.
  const particles = React.useMemo<VizMessage[]>(() => frameMessages
    .filter((message) => message.atMs >= Date.now() - PARTICLE_WINDOW_MS)
    .map((message) => ({ id: message.id, from: message.from, to: message.to, at: message.atMs })), [frameMessages]);
  const memoryEvents = React.useMemo<VizMemoryEvent[]>(() => activity.flatMap((frame) => {
    const op = memoryOp(frame.event);
    const node = op && frame.identity ? baseGraph.resolve(frame.identity) : null;
    return op && node && frame.timestampMs ? [{ id: frame.id, node, op, at: frame.timestampMs }] : [];
  }), [activity, baseGraph]);
  // When each agent's memory was last given to a turn (its newest memory read).
  const memoryGiven = React.useMemo(() => {
    const given = new Map<string, number>();
    for (const event of memoryEvents) {
      if (event.op === "read" && event.at > (given.get(event.node) || 0)) given.set(event.node, event.at);
    }
    return given;
  }, [memoryEvents]);
  const tools = React.useMemo(() => openTools(activity, (reference) => baseGraph.resolve(reference)), [activity, baseGraph]);

  // Editing: the management state decides; the drawer and the editor act.
  const endpoints = React.useMemo<TopologyEndpoint[]>(() => graph.agents.map((agent) => ({
    ref: agent.ref,
    presentation: {
      label: agent.presentation?.label || agent.label,
      caption: agent.presentation?.caption || agent.role,
      section: agent.presentation?.section || (agent.kind === "external" ? agent.authority || "Other teams" : sectionOf(agent)),
      scopeId: agent.presentation?.scopeId,
      scopeLabel: agent.presentation?.scopeLabel,
      crossScope: agent.presentation?.crossScope,
      accent: agent.presentation?.accent,
      searchTerms: [
        ...(agent.presentation?.searchTerms || []),
        agent.role,
        agent.group,
        agent.subgroup || "",
      ].filter(Boolean),
    },
    state: agent.state,
    tags: agent.labels,
  })), [graph.agents, sectionOf]);
  const connectedPairs = React.useMemo(() => graph.edges
    .filter((edge) => (edge.relation === "wired" || edge.relation === "cross") && edge.state !== "declared" && edge.endpoints)
    .map((edge) => edge.endpoints!), [graph.edges]);
  const canRequestGraphMutation = React.useMemo(() => {
    if (!management || management.policy.mode !== "editable" || !onRequestMutation) return false;
    const connected = new Set(connectedPairs.map((edge) => topologyEdgeKey(edge)));
    return management.affordances.some((affordance) => {
      const action: TopologyMutationKind | null = connected.has(topologyEdgeKey(affordance.edge))
        ? "disconnect"
        : affordance.state === "disconnected"
          ? "connect"
          : affordance.preferredAction || null;
      return action !== null && topologyMutationIntent(management, action, affordance.edge, "graph") !== null;
    });
  }, [connectedPairs, management, onRequestMutation]);
  const canConnectFrom = React.useCallback((agent: TopoAgent) => Boolean(
    management && management.policy.mode === "editable" && onRequestMutation && agent.kind !== "external",
  ), [management, onRequestMutation]);

  const openChat = React.useCallback((id: string) => {
    if (graph.byId.get(id)?.kind === "external") return;
    if (graph.byId.has(id)) onSelectAgent?.(id);
  }, [graph.byId, onSelectAgent]);

  const busyCount = model.counts.working;
  const liveCount = Object.values(liveActivity.active).filter((at) => at >= Date.now() - 8_000).length;
  const narrowRef = React.useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = React.useState(1024);
  React.useEffect(() => {
    const element = narrowRef.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const next = entries[0]?.contentRect.width;
      if (next) setWidth(next);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  // In a pane too narrow for the map the default view is the list (the
  // switch keeps Lanes); a view the person picked stays.
  const [pickedThisSession, setPickedThisSession] = React.useState(false);
  const shownView: TopologyPanelView = width < 400 && view === "graph" && !pickedThisSession && controlledView == null ? "list" : view;
  const chooseView = (next: TopologyPanelView) => {
    setPickedThisSession(true);
    pickView(next);
  };

  const viewOptions: SegmentOption<TopologyPanelView>[] = VIEW_OPTIONS.map((option) => ({
    id: option.id,
    label: (
      <span className="topo__view-option" title={option.help}>
        <KitGlyph name={option.glyph} />
        <span>{option.label}</span>
      </span>
    ),
    ariaLabel: option.label,
    testId: `topology-view:${option.id}`,
  }));

  const copyData = () => ({
    agents: graph.agents.map((agent) => ({ id: agent.id, label: agent.label, role: agent.role, kind: agent.kind, state: agent.state })),
    relations: graph.edges.map((edge) => ({ relation: edge.relation, from: edge.from, to: edge.to, state: edge.state, weight: edge.weight })),
    counts: model.counts,
    problems: model.problems.map((problem) => problem.text),
  });

  const panelOptions: MenuEntry[] = [
    {
      kind: "radio",
      id: "window",
      label: "Messages window",
      value: windowId,
      options: (Object.keys(TOPOLOGY_WINDOWS) as TopologyWindowId[]).map((id) => ({
        id,
        label: TOPOLOGY_WINDOWS[id].label,
        testId: `topology-window:${id}`,
      })),
      onChange: (id) => setStoredWindow(isTopologyWindowId(id) ? id : DEFAULT_TOPOLOGY_WINDOW),
    },
    {
      kind: "radio",
      id: "edges",
      label: "Edges",
      value: edgeMode,
      options: [
        { id: "all", label: "All edges", testId: "topology-edges:all" },
        { id: "focus", label: "Hovered or selected agent only", testId: "topology-edges:focus" },
      ],
      onChange: (id) => setEdgeMode(id === "focus" ? "focus" : "all"),
    },
    { kind: "checkbox", id: "helpers", label: "Show helpers", checked: show.helpers, onChange: (checked) => setShowHelpers(checked ? "on" : "off"), testId: "topology-show-helpers" },
    { kind: "checkbox", id: "retired", label: "Show retired", checked: show.retired, onChange: (checked) => setShowRetired(checked ? "on" : "off"), testId: "topology-show-retired" },
    {
      kind: "checkbox",
      id: "channels",
      label: routesAccess === "denied" ? "Show channels (needs routing access)" : "Show channels",
      checked: show.channels && routesAccess !== "denied",
      disabled: routesAccess === "denied",
      onChange: (checked) => setShowChannels(checked ? "on" : "off"),
      testId: "topology-show-channels",
    },
    ...(hasConnectionView ? [{
      kind: "item" as const,
      id: "edit-connections",
      label: "Edit connections",
      onSelect: () => chooseView("connections"),
      testId: "topology-view:connections",
    }] : []),
  ];
  const menuItems = standardPanelMenu({
    viewOptions: panelOptions,
    density,
    onDensityChange: setDensity,
    showTechnicalIds,
    onShowTechnicalIdsChange: setShowTechnicalIds,
    getCopyData: copyData,
    onRefresh,
    testIdPrefix: "topology",
  });

  const summary = topologySummaryFacts(model).map((fact) => ({ key: fact.key, text: fact.text, tone: fact.tone }));
  const noAgents = !loading && graph.agents.length === 0;
  const routesNote = routesAccess === "denied" ? "Routes need routing access" : null;

  const editor = (sourceId: string | null, allowSourceChange: boolean) => management ? (
    <ConnectionPicker
      endpoints={endpoints}
      edges={connectedPairs}
      management={management}
      sourceId={sourceId ?? connectionSourceId}
      defaultSourceId={defaultConnectionSourceId}
      onSourceChange={onConnectionSourceChange}
      onRequestMutation={onRequestMutation}
      onRequestPairInspection={onRequestPairInspection}
      interactionMode={interactionMode}
      resolvingPairKeys={resolvingPairKeys}
      onRetryOperation={onRetryOperation}
      bulkActions={bulkActions}
      onRequestBulkAction={onRequestBulkAction}
      allowSourceChange={allowSourceChange}
      title={allowSourceChange ? "Connections" : "Connect to..."}
    />
  ) : null;

  const connectAgent = connectFor ? graph.byId.get(connectFor) : undefined;
  const drawer = selected ? (
    <TopologyDrawer
      graph={graph}
      target={selected}
      statusOf={(id) => model.statusById.get(id)}
      window={windowId}
      teamName={teamName}
      sectionOf={sectionOf}
      nowOf={(agent) => topologyNowLine(agent, model.statusById.get(agent.id), tools.get(agent.id) || null, now)}
      activityOf={(agent) => topologyAgentDay(activitySource, agent.ref.identity || agent.id, now)}
      memoryOf={(agent) => memoryById.get(agent.ref.identity || agent.id) || null}
      memoryGivenOf={(agent) => memoryGiven.get(agent.id) ?? null}
      management={management}
      onRequestMutation={onRequestMutation}
      canConnect={canConnectFrom}
      onConnectTo={(agent) => {
        setConnectFor(agent.id);
        onConnectionSourceChange?.(agent.ref.id);
      }}
      onOpenChat={onSelectAgent ? (agent) => openChat(agent.id) : undefined}
      onOpenDetails={onOpenAgentDetails ? (agent) => onOpenAgentDetails(agent.id) : undefined}
      onOpenMemory={onOpenMemory ? (agent) => onOpenMemory(agent ? agent.id : null) : undefined}
      onSelect={(target) => {
        setConnectFor(null);
        setSelected(target);
        if (target.kind !== "edge") graphRef.current?.reveal(target.id);
      }}
      onClose={() => {
        setConnectFor(null);
        setSelected(null);
      }}
      editor={connectAgent && selected.kind === "agent" && selected.id === connectAgent.id ? editor(connectAgent.ref.id, false) : undefined}
      now={now}
    />
  ) : null;

  const layout = shownView === "clusters"
    ? model.nodes.length > CLUSTER_THRESHOLD ? "clusters" as const : "force" as const
    : model.nodes.length > CLUSTER_THRESHOLD ? "clusters" as const : "lanes" as const;

  const map = (
    <div className="topo__map" data-testid="topology-map">
      {noAgents ? (
        <EmptyState
          illustration="agents"
          title="No agents in this team yet"
          body="Agents appear here when the team starts them."
          action={onCreateAgent ? <Button variant="primary" onClick={onCreateAgent}>Add an agent</Button> : undefined}
          testId="topology-empty-agents"
        />
      ) : loading && graph.agents.length === 0 ? (
        <LoadingState variant="graph" label="Loading the topology" />
      ) : (
        <>
          <GraphView
            ref={graphRef}
            nodes={model.nodes}
            edges={edges}
            lanes={model.lanes}
            layout={layout}
            density={density}
            ariaLabel={graphSummary(model.nodes, edges)}
            testIdPrefix="topology"
            testIds={{ root: "topology-dense-map" }}
            rootAttributes={{
              "data-topology-editable": canRequestGraphMutation ? "true" : "false",
              "data-drag-kind": "none",
              "data-relations": model.hasRelations ? "true" : "false",
            }}
            selection={selectedNode ? [selectedNode] : []}
            onSelect={(ids) => {
              setConnectFor(null);
              const id = ids[0];
              if (!id) {
                setSelected(null);
                return;
              }
              const kind = graph.byId.has(id) ? "agent" : id.startsWith("store:") ? "store" : "source";
              setSelected({ kind, id });
            }}
            onOpen={openChat}
            selectedEdge={selected?.kind === "edge" ? selected.id : null}
            onEdgeSelect={(id) => {
              setConnectFor(null);
              setSelected(id ? { kind: "edge", id } : null);
            }}
            onHover={(target) => setHovered(target?.node ?? null)}
            messages={particles}
            memoryEvents={memoryEvents}
            legend={model.hasRelations ? "auto" : "off"}
            minimap={model.nodes.length > CLUSTER_THRESHOLD ? "on" : "auto"}
            showRetired={show.retired}
            cacheKey={`topology:${teamKey || "team"}`}
            classNames={{ legendHeading: "topo__viewbar-tag" }}
            legendNote={routesNote ?? undefined}
            className="topo__graph"
          />
          {!model.hasRelations ? (
            <div className="topo__notice" role="status" data-testid="topology-empty-relations">
              <EmptyIllustration name="connections" size="inline" />
              <div className="topo__notice-copy">
                <p className="topo__notice-title">No connections yet</p>
                <p className="topo__notice-body">
                  These agents are not wired to each other, no route feeds them and they have not
                  messaged each other {TOPOLOGY_WINDOWS[windowId].phrase}.
                </p>
                <p className="topo__notice-actions">
                  <a href={wiringDocsHref} target="_blank" rel="noreferrer">How wiring works</a>
                  {canWireMembers && hasConnectionView ? (
                    <button type="button" className="topo__notice-link" onClick={() => chooseView("connections")}>Connect agents</button>
                  ) : null}
                </p>
              </div>
            </div>
          ) : null}
        </>
      )}
    </div>
  );

  let body: React.ReactNode;
  if (shownView === "roles") {
    body = (
      <RoleTree
        graph={graph}
        statusById={model.statusById}
        onSelectAgent={onSelectAgent}
        teamName={teamName}
        teamKey={teamKey}
      />
    );
  } else if (shownView === "connections" && management) {
    body = <div className="topo__editor">{editor(null, true)}</div>;
  } else if (shownView === "list") {
    body = (
      <DrawerLayout
        main={(
          <GraphList
            nodes={model.nodes}
            edges={listEdges}
            lanes={model.lanes}
            ariaLabel="Agents and their connections"
            selection={selectedNode ? [selectedNode] : []}
            onSelect={(ids) => {
              const id = ids[0];
              setConnectFor(null);
              setSelected(id ? { kind: graph.byId.has(id) ? "agent" : id.startsWith("store:") ? "store" : "source", id } : null);
            }}
            onOpen={openChat}
            testIdPrefix="topology"
          />
        )}
        drawer={drawer}
        open={Boolean(drawer)}
        onClose={() => setSelected(null)}
        persistKey="topology"
        testId="topology-layout"
      />
    );
  } else {
    body = (
      <DrawerLayout
        main={map}
        drawer={drawer}
        open={Boolean(drawer)}
        onClose={() => setSelected(null)}
        persistKey="topology"
        testId="topology-layout"
      />
    );
  }

  return (
    <div
      ref={narrowRef}
      className="topo"
      data-testid="topology-panel"
      data-activity-count={activity.length}
      data-busy-count={busyCount}
      data-live-count={liveCount}
      data-management-mode={management?.policy.mode || "unavailable"}
      data-view={shownView}
    >
      <PanelShell
        className="topo__frame"
        density={density}
        showTechnicalIds={showTechnicalIds}
        ariaLabel="Topology"
      >
        <PanelHeader
          title={title}
          scope={teamName ? <Chip variant="quiet" title={teamKey}>{teamName}</Chip> : undefined}
          summary={summary}
          freshness={{ at: updatedAtMs ?? undefined, updating: refreshing }}
          refreshing={refreshing}
          view={{
            ariaLabel: "View",
            options: viewOptions,
            value: shownView,
            onChange: (next) => chooseView(next as TopologyPanelView),
            testId: "topology-view",
          }}
          tools={onRefresh ? (
            <IconButton label="Refresh topology" glyph="refresh" reveal onClick={onRefresh} data-testid="topology-refresh" />
          ) : undefined}
          menu={{ label: "More actions for Topology", items: menuItems, triggerTestId: "topology-menu" }}
        />
        {!noAgents ? (
          <TopologyGlance
            counts={model.counts}
            problems={model.problems}
            traffic={trafficWindow}
            window={windowId}
            trafficKnown
            loading={loading && graph.agents.length === 0}
            narrow={width < 360}
          />
        ) : null}
        <PanelBody className="topo__body">
          {error || liveUnavailable ? (
            <StatusToast
              error={LIVE_UNAVAILABLE}
              errorTestId="topology-error"
              updatedAt={updatedAtMs ?? undefined}
              onRetry={onRefresh}
            />
          ) : null}
          {body}
        </PanelBody>
      </PanelShell>
    </div>
  );
}
