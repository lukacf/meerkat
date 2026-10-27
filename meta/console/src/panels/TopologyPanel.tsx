import React from "react";
import type { ConsoleAgent, ConsoleFrame, ConsoleTopologyNode } from "../types";
import { RoleTree } from "./topology/RoleTree";
import { DenseGraphMap } from "./topology/DenseGraphMap";
import { buildGraph, useTopologyActivity } from "./topology/data";

interface TopologyPanelProps {
  nodes: ConsoleTopologyNode[];
  agents: ConsoleAgent[];
  activity: ConsoleFrame[];
}

type View = "graph" | "roles";
type EdgeMode = "all" | "focus";
const VIEW_STORAGE = "meta-console-topology-view";
const EDGE_STORAGE = "meta-console-topology-edges";
const VIEWS: Array<{ id: View; label: string; help: string }> = [
  { id: "graph", label: "Graph", help: "Dense canvas graph with every node in one view" },
  { id: "roles", label: "Roles", help: "Flat mob · agents grouped by role" },
];
const EDGE_MODES: Array<{ id: EdgeMode; label: string; help: string }> = [
  { id: "all", label: "All", help: "Draw all graph edges persistently" },
  { id: "focus", label: "Focus", help: "Show only hovered-agent edges" },
];

export function TopologyPanel({
  nodes,
  agents,
  activity,
}: TopologyPanelProps): React.JSX.Element {
  const [view, setView] = React.useState<View>(() => {
    try {
      const stored = localStorage.getItem(VIEW_STORAGE);
      if (stored === "summary") return "graph";
      if (stored === "force") return "graph";
      if (stored === "bullseye") return "graph";
      if (stored === "graph" || stored === "roles") return stored;
    } catch { /* ignore */ }
    return "graph";
  });
  const pickView = (next: View) => {
    setView(next);
    try { localStorage.setItem(VIEW_STORAGE, next); } catch { /* ignore */ }
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
  const live = useTopologyActivity(activity, graph, { life: 8000 });
  const liveCount = Object.keys(live.active).length;
  const busyCount = Object.values(live.busy).filter(Boolean).length;

  return (
    <div
      className="topo"
      data-testid="topology-panel"
      data-activity-count={activity.length}
      data-busy-count={busyCount}
      data-live-count={liveCount}
    >
      <div className="topo__head">
        <h2>Topology</h2>
        <span className="topo__head-meta">
          {graph.agents.length} agents · {graph.edges.length} edges
          {busyCount > 0 ? ` · ${busyCount} working` : ""}
          {liveCount > 0 && busyCount === 0 ? ` · ${liveCount} live` : ""}
        </span>
        {view === "graph" && (
          <div className="topo__viewbar topo__viewbar--labels" role="group" aria-label="Edges">
            <span className="topo__viewbar-tag">Edges</span>
            {EDGE_MODES.map((m) => (
              <button
                key={m.id}
                type="button"
                className={`topo__viewbtn ${edgeMode === m.id ? "is-active" : ""}`}
                onClick={() => pickEdgeMode(m.id)}
                title={m.help}
                data-testid={`topology-edges:${m.id}`}
              >
                {m.label}
              </button>
            ))}
          </div>
        )}
        <div className="topo__viewbar">
          {VIEWS.map((v) => (
            <button
              key={v.id}
              type="button"
              className={`topo__viewbtn ${view === v.id ? "is-active" : ""}`}
              onClick={() => pickView(v.id)}
              title={v.help}
              data-testid={`topology-view:${v.id}`}
            >
              {v.label}
            </button>
          ))}
        </div>
      </div>
      <div className="topo__body">
        {view === "graph" && <DenseGraphMap graph={graph} edgeMode={edgeMode} activity={live} />}
        {view === "roles" && (
          <RoleTree
            nodes={nodes}
            agents={agents}
            activity={activity}
          />
        )}
      </div>
    </div>
  );
}
