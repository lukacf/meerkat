import React from "react";

import {
  topologyEdgeKey,
  topologyMutationIntent,
  topologyOperationIsPending,
  type TopologyEndpoint,
  type TopologyManagementState,
  type TopologyMutationIntent,
  type TopologyMutationKind,
  type TopologyMutationOrigin,
  type TopologyOperationReceipt,
} from "@console-core";

import type {
  ConsoleAgent,
  ConsoleFrame,
  ConsoleTopologyNode,
  TopologyEdgeRef,
  TopologyPanelView,
} from "./types";
import { RoleTree } from "./role-tree";
import { DenseGraphMap } from "./dense-graph-map";
import { buildGraph, useTopologyActivity } from "./data";
import {
  ConnectionPicker,
  type TopologyBoundedAction,
} from "./connection-picker";

export interface TopologyPanelProps {
  nodes: ConsoleTopologyNode[];
  agents: ConsoleAgent[];
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
}

type EdgeMode = "all" | "focus";
const VIEW_STORAGE = "meta-console-topology-view";
const EDGE_STORAGE = "meta-console-topology-edges";
const VIEWS: Array<{ id: TopologyPanelView; label: string; help: string }> = [
  { id: "graph", label: "Graph", help: "Dense canvas graph with every node in one view" },
  { id: "roles", label: "Roles", help: "Flat roster grouped by role" },
  { id: "connections", label: "Connections", help: "Search-first connection roster" },
];
const EDGE_MODES: Array<{ id: EdgeMode; label: string; help: string }> = [
  { id: "all", label: "All", help: "Draw all graph edges persistently" },
  { id: "focus", label: "Focus", help: "Show only hovered-agent edges" },
];

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
}: TopologyPanelProps): React.JSX.Element {
  const hasConnectionView = Boolean(management && management.policy.mode !== "disabled");
  const [uncontrolledView, setUncontrolledView] = React.useState<TopologyPanelView>(() => {
    if (defaultView && (defaultView !== "connections" || hasConnectionView)) return defaultView;
    try {
      const stored = localStorage.getItem(VIEW_STORAGE);
      if (stored === "edit" && hasConnectionView) return "connections";
      if (stored === "summary" || stored === "force" || stored === "bullseye") return "graph";
      if (stored === "graph" || stored === "roles" || (stored === "connections" && hasConnectionView)) {
        return stored;
      }
    } catch { /* ignore */ }
    return "graph";
  });
  const requestedView = controlledView ?? uncontrolledView;
  const view = requestedView === "connections" && !hasConnectionView ? "graph" : requestedView;
  const pickView = (next: TopologyPanelView) => {
    if (next === "connections" && !hasConnectionView) return;
    if (controlledView == null) {
      setUncontrolledView(next);
      try { localStorage.setItem(VIEW_STORAGE, next); } catch { /* ignore */ }
    }
    onViewChange?.(next);
  };

  const [edgeMode, setEdgeMode] = React.useState<EdgeMode>(() => {
    try {
      const stored = localStorage.getItem(EDGE_STORAGE);
      if (stored === "all" || stored === "focus") return stored;
    } catch { /* ignore */ }
    return "all";
  });
  const pickEdgeMode = (next: EdgeMode) => {
    setEdgeMode(next);
    try { localStorage.setItem(EDGE_STORAGE, next); } catch { /* ignore */ }
  };

  const graph = React.useMemo(() => buildGraph(nodes, agents), [nodes, agents]);
  const endpoints = React.useMemo<TopologyEndpoint[]>(() => graph.agents.map((agent) => ({
    ref: agent.ref,
    presentation: {
      label: agent.presentation?.label || agent.label,
      caption: agent.presentation?.caption || agent.role,
      section: agent.presentation?.section || agent.group,
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
  })), [graph.agents]);
  const live = useTopologyActivity(activity, graph, { life: 8000 });
  const liveCount = Object.keys(live.active).length;
  const busyCount = Object.values(live.busy).filter(Boolean).length;
  const pendingEdgeKeys = React.useMemo(() => new Set(
    (management?.operations || [])
      .filter((operation) => operation.edge && topologyOperationIsPending(operation))
      .map((operation) => topologyEdgeKey(operation.edge!)),
  ), [management?.operations]);

  const resolveMutation = React.useCallback((
    action: TopologyMutationKind,
    edge: TopologyEdgeRef,
    origin: TopologyMutationOrigin,
  ): TopologyMutationIntent | null => (
    management ? topologyMutationIntent(management, action, edge, origin) : null
  ), [management]);
  const canRequestGraphMutation = React.useMemo(() => {
    if (!management || management.policy.mode !== "editable" || !onRequestMutation) return false;
    const connectedEdgeKeys = new Set(graph.edges.map((edge) => topologyEdgeKey(edge)));
    return management.affordances.some((affordance) => {
      const action = connectedEdgeKeys.has(topologyEdgeKey(affordance.edge))
        ? "disconnect"
        : affordance.state === "disconnected"
          ? "connect"
          : null;
      return action !== null
        && topologyMutationIntent(management, action, affordance.edge, "graph") !== null;
    });
  }, [graph.edges, management, onRequestMutation]);

  return (
    <div
      className="topo"
      data-testid="topology-panel"
      data-activity-count={activity.length}
      data-busy-count={busyCount}
      data-live-count={liveCount}
      data-management-mode={management?.policy.mode || "unavailable"}
    >
      <div className="topo__head">
        <h2>{title}</h2>
        <span className="topo__head-meta">
          {graph.agents.length} agents · {graph.edges.length} links
          {busyCount > 0 ? ` · ${busyCount} working` : ""}
          {liveCount > 0 && busyCount === 0 ? ` · ${liveCount} live` : ""}
          {management?.policy.mode === "read_only" ? " · read-only" : ""}
        </span>
        {view === "graph" ? (
          <div className="topo__viewbar topo__viewbar--labels" role="group" aria-label="Edges">
            <span className="topo__viewbar-tag">Edges</span>
            {EDGE_MODES.map((mode) => (
              <button
                key={mode.id}
                type="button"
                className={`topo__viewbtn ${edgeMode === mode.id ? "is-active" : ""}`}
                onClick={() => pickEdgeMode(mode.id)}
                title={mode.help}
                data-testid={`topology-edges:${mode.id}`}
              >
                {mode.label}
              </button>
            ))}
          </div>
        ) : null}
        <div className="topo__viewbar">
          {VIEWS.filter((candidate) => candidate.id !== "connections" || hasConnectionView).map((candidate) => (
            <button
              key={candidate.id}
              type="button"
              className={`topo__viewbtn ${view === candidate.id ? "is-active" : ""}`}
              onClick={() => pickView(candidate.id)}
              title={candidate.help}
              data-testid={`topology-view:${candidate.id}`}
            >
              {candidate.label}
            </button>
          ))}
        </div>
      </div>
      <div className="topo__body">
        {view === "graph" ? (
          <DenseGraphMap
            graph={graph}
            edgeMode={edgeMode}
            activity={live}
            pendingEdgeKeys={pendingEdgeKeys}
            canRequestMutation={canRequestGraphMutation}
            resolveMutation={management?.policy.mode === "editable" ? resolveMutation : undefined}
            onRequestMutation={onRequestMutation}
          />
        ) : null}
        {view === "roles" ? (
          <RoleTree
            nodes={nodes}
            agents={agents}
            activity={activity}
          />
        ) : null}
        {view === "connections" && management ? (
          <ConnectionPicker
            endpoints={endpoints}
            edges={graph.edges}
            management={management}
            sourceId={connectionSourceId}
            defaultSourceId={defaultConnectionSourceId}
            onSourceChange={onConnectionSourceChange}
            onRequestMutation={onRequestMutation}
            onRequestPairInspection={onRequestPairInspection}
            interactionMode={interactionMode}
            resolvingPairKeys={resolvingPairKeys}
            onRetryOperation={onRetryOperation}
            bulkActions={bulkActions}
            onRequestBulkAction={onRequestBulkAction}
          />
        ) : null}
      </div>
    </div>
  );
}
